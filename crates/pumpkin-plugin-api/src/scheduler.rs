//! Task scheduling API for plugins.
//!
//! This module provides the ability for plugins to schedule closures to be
//! executed after a delay or repeatedly in the server's main tick loop.
//!
//! # Example
//!
//! ```rust,ignore
//! use pumpkin_plugin_api::scheduler::SchedulerExt;
//!
//! context.schedule_delayed_task(20, |server| {
//!     server.log("One second has passed!");
//! });
//! ```
//!
//! # Worker jobs
//!
//! Work that takes long, such as building a large file, can run in a worker instance of the
//! plugin with [`submit_worker_job`], so the plugin keeps handling events while the job runs.
//!
//! ```rust,ignore
//! use pumpkin_plugin_api::{Plugin, scheduler::submit_worker_job};
//!
//! impl Plugin for MyPlugin {
//!     // ...
//!     fn run_worker_job(&self, kind: &str, input: Vec<u8>) -> Result<Vec<u8>, String> {
//!         match kind {
//!             "compress" => Ok(compress(&input)),
//!             _ => Err(format!("unknown worker job `{kind}`")),
//!         }
//!     }
//! }
//!
//! submit_worker_job("compress", &data, |server, result| match result {
//!     Ok(compressed) => { /* use the output with the server */ }
//!     Err(error) => tracing::warn!("compressing failed: {error}"),
//! })?;
//! ```

use crate::wit::pumpkin::plugin::context::Server;
use crate::wit::pumpkin::plugin::scheduler;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A type alias for a closure that can be scheduled as a task.
///
/// Scheduled callbacks can be re-entered, so mutable captured state must use thread-safe
/// interior mutability.
pub type TaskHandler = Arc<dyn Fn(Server) + Send + Sync>;

pub(crate) struct Task {
    handler: TaskHandler,
}

pub(crate) static TASK_HANDLERS: Mutex<LazyTaskHandlers> = Mutex::new(LazyTaskHandlers {
    handlers: BTreeMap::new(),
    next_id: 0,
});

pub(crate) struct LazyTaskHandlers {
    handlers: BTreeMap<u32, Task>,
    next_id: u32,
}

impl LazyTaskHandlers {
    /// Registers a new task handler and returns its unique ID.
    pub fn register(&mut self, handler: TaskHandler) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.handlers.insert(id, Task { handler });
        id
    }

    /// Returns the task handler for the given ID.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<TaskHandler> {
        self.handlers.get(&id).map(|task| Arc::clone(&task.handler))
    }
}

/// Extension trait to provide ergonomic task scheduling on `Context` and `Server`.
pub trait SchedulerExt {
    /// Schedules a task to be executed once after the specified number of ticks.
    ///
    /// * `delay_ticks`: Number of game ticks to wait before execution.
    /// * `handler`: Closure to execute.
    ///
    /// Returns a unique task ID.
    fn schedule_delayed_task<F>(&self, delay_ticks: u64, handler: F) -> u32
    where
        F: Fn(Server) + Send + Sync + 'static;

    /// Schedules a task to be executed repeatedly.
    ///
    /// * `delay_ticks`: Number of game ticks to wait before the first execution.
    /// * `period_ticks`: Number of ticks between subsequent executions.
    /// * `handler`: Closure to execute.
    ///
    /// Returns a unique task ID.
    fn schedule_repeating_task<F>(&self, delay_ticks: u64, period_ticks: u64, handler: F) -> u32
    where
        F: Fn(Server) + Send + Sync + 'static;
}

impl SchedulerExt for crate::Context {
    fn schedule_delayed_task<F>(&self, delay_ticks: u64, handler: F) -> u32
    where
        F: Fn(Server) + Send + Sync + 'static,
    {
        schedule_delayed_task(delay_ticks, handler)
    }

    fn schedule_repeating_task<F>(&self, delay_ticks: u64, period_ticks: u64, handler: F) -> u32
    where
        F: Fn(Server) + Send + Sync + 'static,
    {
        schedule_repeating_task(delay_ticks, period_ticks, handler)
    }
}

impl SchedulerExt for crate::Server {
    fn schedule_delayed_task<F>(&self, delay_ticks: u64, handler: F) -> u32
    where
        F: Fn(Self) + Send + Sync + 'static,
    {
        schedule_delayed_task(delay_ticks, handler)
    }

    fn schedule_repeating_task<F>(&self, delay_ticks: u64, period_ticks: u64, handler: F) -> u32
    where
        F: Fn(Self) + Send + Sync + 'static,
    {
        schedule_repeating_task(delay_ticks, period_ticks, handler)
    }
}

/// Lower-level function to schedule a delayed task.
/// Prefer using [`SchedulerExt`] for a more ergonomic API.
pub fn schedule_delayed_task<F>(delay_ticks: u64, handler: F) -> u32
where
    F: Fn(Server) + Send + Sync + 'static,
{
    let handler_id = TASK_HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register(Arc::new(handler));
    scheduler::schedule_delayed_task(handler_id, delay_ticks)
}

/// Lower-level function to schedule a repeating task.
/// Prefer using [`SchedulerExt`] for a more ergonomic API.
pub fn schedule_repeating_task<F>(delay_ticks: u64, period_ticks: u64, handler: F) -> u32
where
    F: Fn(Server) + Send + Sync + 'static,
{
    let handler_id = TASK_HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register(Arc::new(handler));
    scheduler::schedule_repeating_task(handler_id, delay_ticks, period_ticks)
}

/// Cancels a scheduled task.
pub fn cancel_task(task_id: u32) {
    scheduler::cancel_task(task_id);
}

/// A callback that receives the server and the outcome of a worker job.
pub type WorkerJobHandler = Box<dyn FnOnce(Server, Result<Vec<u8>, String>) + Send>;

static WORKER_JOB_HANDLERS: Mutex<BTreeMap<u32, WorkerJobHandler>> = Mutex::new(BTreeMap::new());

/// Runs a job in a worker instance of this plugin, off the server threads.
///
/// The host calls [`Plugin::run_worker_job`](crate::Plugin::run_worker_job) with `kind` and
/// `input` on a separate instance of the plugin, then calls `on_done` on this instance with its
/// outcome, like any other callback.
///
/// The worker instance shares no memory with this one: statics and anything set up in
/// [`Plugin::on_load`](crate::Plugin::on_load) are not there, and only logging works among the
/// host APIs. Pass everything else the job needs in `input`. If the plugin has the
/// `fs.read.data` or `fs.write.data` permission, the job can read (but not write) files in the
/// plugin's data folder, at the same path as on this instance. Each running job costs one more
/// instance of the plugin's memory.
///
/// Returns the job ID, or an error if the host does not run worker jobs for this plugin.
pub fn submit_worker_job<F>(kind: &str, input: &[u8], on_done: F) -> Result<u32, String>
where
    F: FnOnce(Server, Result<Vec<u8>, String>) + Send + 'static,
{
    let job_id = scheduler::submit_worker_job(kind, input)?;
    WORKER_JOB_HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(job_id, Box::new(on_done));
    Ok(job_id)
}

/// Hands the outcome of a worker job to the callback it was submitted with.
pub(crate) fn finish_worker_job(job_id: u32, server: Server, result: Result<Vec<u8>, String>) {
    let handler = WORKER_JOB_HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&job_id);
    if let Some(handler) = handler {
        handler(server, result);
    }
}
