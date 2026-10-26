use crate::{JobResultParams, WORKER_INTERFACE, pumpkin::plugin::scheduler};
use pumpkin_core::server::Server;
use pumpkin_wasm_host_common::{
    plugin::WasmPlugin,
    state::PluginHostState,
    worker::{JobOutcome, WorkerSetup},
};
use std::sync::{Arc, Weak, atomic::Ordering};
use wasmtime::component::Resource;

impl scheduler::Host for PluginHostState {
    fn schedule_delayed_task(&mut self, handler_id: u32, delay: u64) -> wasmtime::Result<u32> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin not found"))?;
        let server = self
            .server
            .as_ref()
            .ok_or_else(|| wasmtime::Error::msg("Server not found"))?;
        let tick_count = server.tick_count.load(Ordering::Relaxed) as u64;
        let task_scheduler = Arc::clone(&plugin.task_scheduler);
        let task_id = task_scheduler.schedule_delayed_task(plugin, handler_id, delay, tick_count);
        Ok(task_id)
    }

    fn schedule_repeating_task(
        &mut self,
        handler_id: u32,
        delay: u64,
        period: u64,
    ) -> wasmtime::Result<u32> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin not found"))?;
        let server = self
            .server
            .as_ref()
            .ok_or_else(|| wasmtime::Error::msg("Server not found"))?;
        let tick_count = server.tick_count.load(Ordering::Relaxed) as u64;
        let task_scheduler = Arc::clone(&plugin.task_scheduler);
        let task_id =
            task_scheduler.schedule_repeating_task(plugin, handler_id, delay, period, tick_count);
        Ok(task_id)
    }

    fn cancel_task(&mut self, task_id: u32) -> wasmtime::Result<()> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin not found"))?;
        plugin.task_scheduler.cancel_task(task_id);
        Ok(())
    }

    fn submit_worker_job(
        &mut self,
        kind: String,
        input: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, String>> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin not found"))?;
        let server = self
            .server
            .clone()
            .ok_or_else(|| wasmtime::Error::msg("Server not found"))?;
        let (Some(worker), Some(_)) = (&plugin.worker, self.worker_job_result_handler) else {
            return Ok(Err(format!(
                "the plugin does not export `{WORKER_INTERFACE}`"
            )));
        };
        let weak_plugin = Arc::downgrade(&plugin);
        Ok(worker.submit(
            kind,
            input,
            WorkerSetup::from_main(self),
            move |job_id, outcome| deliver_job_result(weak_plugin, server, job_id, outcome),
        ))
    }
}

/// Hands a worker job's outcome to the plugin's `worker.handle-job-result`.
///
/// Runs as a root call on the plugin's store, like a scheduled task.
async fn deliver_job_result(
    plugin: Weak<WasmPlugin>,
    server: Arc<Server>,
    job_id: u32,
    outcome: JobOutcome,
) {
    let Some(plugin) = plugin.upgrade() else {
        tracing::debug!(job_id, "Dropping a worker job result of an unloaded plugin");
        return;
    };
    let delivered = plugin
        .store
        .call_guest(move |mut guest| {
            Box::pin(async move {
                let handler = guest
                    .with(|mut store| store.data_mut().worker_job_result_handler)
                    .ok_or_else(|| wasmtime::Error::msg("The plugin has no worker job handler"))?;
                let (server_resource, server_rep) = guest.with(|mut store| {
                    let resource = store.data_mut().add(server)?;
                    let rep = resource.rep();
                    Ok::<_, wasmtime::Error>((resource, rep))
                })?;
                let result = guest
                    .call_func::<JobResultParams, ()>(handler, (job_id, server_resource, outcome))
                    .await;
                guest.with(|mut store| {
                    let _ = store
                        .data_mut()
                        .resource_table
                        .delete::<Arc<Server>>(Resource::new_own(server_rep));
                });
                result
            })
        })
        .await;
    if let Err(error) = delivered {
        tracing::warn!(job_id, %error, "Failed to deliver a Wasm plugin worker job result");
    }
}
