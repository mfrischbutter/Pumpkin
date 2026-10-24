//! Worker instances for heavy plugin jobs.
//!
//! A plugin that exports the `worker` interface can hand a job to a second instance of its own
//! component. That instance runs on Tokio's blocking pool, outside the admission of the plugin's
//! store, so the main instance keeps handling events, commands and tasks while the job runs, and
//! no server thread waits for it.
//!
//! A worker instance shares nothing with the main instance. It has its own linear memory, gets
//! `init-plugin` but never `on-load`, and every Pumpkin import except logging traps there, so a
//! job takes and returns plain bytes. Each running job costs one more instance of the plugin: its
//! linear memory and tables, up to the memory limit of the plugin's main store. The compiled code
//! is shared with the main instance. At most [`MAX_RUNNING_JOBS`] jobs of one plugin run at a
//! time; further jobs wait for a free slot. A job cannot be cancelled, and the result of a job
//! that ends after its plugin unloaded is dropped.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use tokio::sync::Semaphore;
use wasmtime::{
    AsContextMut, Store, StoreLimits,
    component::{Component, Func, Instance, InstancePre, Linker},
};

use crate::state::PluginHostState;

/// How many worker jobs of one plugin run at the same time.
pub const MAX_RUNNING_JOBS: usize = 2;

/// The world export the host calls once on every new instance.
const INIT_PLUGIN: &str = "init-plugin";
/// The function of the `worker` interface that runs a job on a worker instance.
const RUN_JOB: &str = "run-job";
/// The function of the `worker` interface that receives a job's outcome on the main instance.
const HANDLE_JOB_RESULT: &str = "handle-job-result";

/// The output of a worker job, or why it failed.
pub type JobOutcome = Result<Vec<u8>, String>;

/// Runs worker jobs for one plugin whose component exports the `worker` interface.
pub struct PluginWorker {
    instance_pre: InstancePre<PluginHostState>,
    interface: &'static str,
    next_job_id: AtomicU32,
    job_slots: Arc<Semaphore>,
}

impl PluginWorker {
    /// Whether `component` exports `run-job` in its `interface` export.
    #[must_use]
    pub fn is_exported_by(component: &Component, interface: &str) -> bool {
        component
            .get_export_index(None, interface)
            .is_some_and(|exports| component.get_export_index(Some(&exports), RUN_JOB).is_some())
    }

    /// Prepares worker instances of `component`.
    ///
    /// `linker` defines the imports that work on a worker instance and traps for all others, see
    /// `Linker::define_unknown_imports_as_traps`.
    ///
    /// # Errors
    /// Returns an error if `component` cannot be instantiated with `linker`.
    pub fn new(
        linker: &Linker<PluginHostState>,
        component: &Component,
        interface: &'static str,
    ) -> wasmtime::Result<Self> {
        Ok(Self {
            instance_pre: linker.instantiate_pre(component)?,
            interface,
            next_job_id: AtomicU32::new(0),
            job_slots: Arc::new(Semaphore::new(MAX_RUNNING_JOBS)),
        })
    }

    /// Starts a job on a worker instance and returns its ID right away.
    ///
    /// `limits` are the resource limits of the new instance and `name` is the plugin name its
    /// log lines carry. `deliver` receives the job ID and the outcome once the job has finished.
    ///
    /// # Errors
    /// Returns an error if no Tokio runtime is available to run the job on.
    pub fn submit<F, Fut>(
        &self,
        kind: String,
        input: Vec<u8>,
        limits: StoreLimits,
        name: Option<String>,
        deliver: F,
    ) -> Result<u32, String>
    where
        F: FnOnce(u32, JobOutcome) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| format!("worker jobs need a Tokio runtime: {error}"))?;
        let job_id = self.next_job_id.fetch_add(1, Ordering::Relaxed);
        let job = WorkerJob {
            instance_pre: self.instance_pre.clone(),
            interface: self.interface,
            kind,
            input,
            limits,
            name,
        };
        let job_slots = Arc::clone(&self.job_slots);
        let handle = runtime.clone();
        drop(runtime.spawn(async move {
            let outcome = run_in_slot(&job_slots, &handle, job).await;
            if let Err(error) = &outcome {
                tracing::debug!(job_id, %error, "Wasm plugin worker job failed");
            }
            deliver(job_id, outcome).await;
        }));
        Ok(job_id)
    }
}

/// Everything one job needs on the blocking pool.
struct WorkerJob {
    instance_pre: InstancePre<PluginHostState>,
    interface: &'static str,
    kind: String,
    input: Vec<u8>,
    limits: StoreLimits,
    name: Option<String>,
}

/// Waits for a free job slot of the plugin, then runs the job on the blocking pool.
async fn run_in_slot(
    job_slots: &Arc<Semaphore>,
    runtime: &tokio::runtime::Handle,
    job: WorkerJob,
) -> JobOutcome {
    let slot = Arc::clone(job_slots)
        .acquire_owned()
        .await
        .map_err(|_| "the plugin's worker job slots are closed".to_owned())?;
    let blocking_runtime = runtime.clone();
    let outcome = runtime
        .spawn_blocking(move || {
            // Wasm runs inside the polled future, so it runs on this blocking thread.
            let outcome = blocking_runtime
                .block_on(run_job(job))
                .unwrap_or_else(|error| Err(format!("{error:#}")));
            drop(slot);
            outcome
        })
        .await;
    outcome.unwrap_or_else(|error| Err(format!("the worker job panicked: {error}")))
}

/// Runs one job on a fresh worker instance.
///
/// The outer error is a failure of the instance itself, such as a trap; the inner one is the
/// error `run-job` returned.
async fn run_job(job: WorkerJob) -> wasmtime::Result<JobOutcome> {
    let mut state = PluginHostState::new();
    state.limits = job.limits;
    state.name = job.name;
    let mut store = Store::new(job.instance_pre.engine(), state);
    store.limiter(|state| &mut state.limits);

    let instance = job.instance_pre.instantiate_async(&mut store).await?;
    instance
        .get_typed_func::<(), ()>(&mut store, INIT_PLUGIN)?
        .call_async(&mut store, ())
        .await?;

    let exports = instance
        .get_export_index(&mut store, None, job.interface)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing `{}` export", job.interface)))?;
    let run = instance
        .get_export_index(&mut store, Some(&exports), RUN_JOB)
        .ok_or_else(|| wasmtime::Error::msg(format!("missing `{RUN_JOB}` export")))?;
    let (outcome,) = instance
        .get_typed_func::<(String, Vec<u8>), (JobOutcome,)>(&mut store, &run)?
        .call_async(&mut store, (job.kind, job.input))
        .await?;
    Ok(outcome)
}

/// Looks up `handle-job-result` of the `interface` export on a plugin's main instance.
///
/// Returns `None` for a plugin that does not export the interface.
#[must_use]
pub fn job_result_handler(
    mut store: impl AsContextMut,
    instance: &Instance,
    interface: &str,
) -> Option<Func> {
    let exports = instance.get_export_index(&mut store, None, interface)?;
    let handler = instance.get_export_index(&mut store, Some(&exports), HANDLE_JOB_RESULT)?;
    instance.get_func(&mut store, &handler)
}
