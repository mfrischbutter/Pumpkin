use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const MAX_SYNC_REENTRY_DEPTH: usize = 64;

/// How often a chain waiting for a Store looks at the wait graph again.
///
/// A wait that closes a cycle is caught when it starts. The re-check covers a cycle closed by a
/// forked chain taking a Store while another of its branches already waits.
const WAIT_CYCLE_RECHECK_INTERVAL: Duration = Duration::from_millis(50);

static NEXT_ROOT_ADMISSION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_STORE_GATE_ID: AtomicU64 = AtomicU64::new(1);

/// The admission authority shared by all Stores of one plugin call graph.
///
/// It hands out causal chain ids and records which chain holds which Store and which chains
/// wait for a Store, so a wait that could never end is noticed.
#[derive(Clone, Debug)]
pub struct RootAdmission {
    state: Arc<RootAdmissionState>,
}

#[derive(Debug)]
struct RootAdmissionState {
    id: u64,
    next_chain_id: AtomicU64,
    graph: StdMutex<WaitGraph>,
}

impl RootAdmissionState {
    fn graph(&self) -> MutexGuard<'_, WaitGraph> {
        self.graph.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Lets one causal chain at a time into one Store.
#[derive(Debug)]
pub struct StoreGate {
    id: u64,
    permits: Arc<Semaphore>,
}

impl StoreGate {
    pub fn new() -> Self {
        Self {
            id: NEXT_STORE_GATE_ID.fetch_add(1, Ordering::Relaxed),
            permits: Arc::new(Semaphore::new(1)),
        }
    }
}

/// A causal chain's hold on one Store. Dropping it lets the next waiting chain in.
pub struct StoreAdmission {
    state: Arc<RootAdmissionState>,
    gate_id: u64,
    context: ReentryContext,
    acquired_at: Instant,
    permit: Option<OwnedSemaphorePermit>,
}

/// How a causal chain that already runs elsewhere gets into a Store.
pub enum ChainAdmission {
    /// The chain already holds the Store.
    Held,
    /// The chain now holds the Store.
    Admitted(StoreAdmission),
    /// The Store is held by `holder_chain_id`, which waits, directly or through other chains, on
    /// a Store this chain holds. Neither chain can go on by waiting.
    Cycle { holder_chain_id: u64 },
}

/// Which chain holds which Store gate, and which chains wait for one.
#[derive(Debug, Default)]
struct WaitGraph {
    /// The chain each held gate was admitted for, by gate id.
    holders: HashMap<u64, u64>,
    /// Chains waiting for a gate, by wait id.
    waits: HashMap<u64, Wait>,
    next_wait_id: u64,
}

#[derive(Clone, Copy, Debug)]
struct Wait {
    chain_id: u64,
    gate_id: u64,
}

impl WaitGraph {
    fn holder(&self, gate_id: u64) -> Option<u64> {
        self.holders.get(&gate_id).copied()
    }

    /// Returns the chain holding `gate_id` if it waits, directly or through other chains, on a
    /// gate held by `chain_id`.
    fn cycle_holder(&self, chain_id: u64, gate_id: u64) -> Option<u64> {
        let holder = self.holder(gate_id)?;
        let mut visited = HashSet::new();
        let mut pending = vec![holder];
        while let Some(waiting_chain) = pending.pop() {
            if waiting_chain == chain_id {
                return Some(holder);
            }
            if !visited.insert(waiting_chain) {
                continue;
            }
            pending.extend(
                self.waits
                    .values()
                    .filter(|wait| wait.chain_id == waiting_chain)
                    .filter_map(|wait| self.holder(wait.gate_id)),
            );
        }
        None
    }

    fn add_wait(&mut self, chain_id: u64, gate_id: u64) -> u64 {
        let wait_id = self.next_wait_id;
        self.next_wait_id += 1;
        self.waits.insert(wait_id, Wait { chain_id, gate_id });
        wait_id
    }

    fn remove_wait(&mut self, wait_id: u64) {
        self.waits.remove(&wait_id);
    }

    fn hold(&mut self, gate_id: u64, chain_id: u64) {
        self.holders.insert(gate_id, chain_id);
    }

    fn release(&mut self, gate_id: u64, chain_id: u64) {
        if self.holder(gate_id) == Some(chain_id) {
            self.holders.remove(&gate_id);
        }
    }
}

/// Removes a chain's wait from the graph when the wait ends, however it ends.
struct WaitRegistration {
    state: Arc<RootAdmissionState>,
    wait_id: u64,
}

impl Drop for WaitRegistration {
    fn drop(&mut self) {
        self.state.graph().remove_wait(self.wait_id);
    }
}

fn admission_closed() -> wasmtime::Error {
    wasmtime::Error::msg("Wasm plugin store admission is closed")
}

impl RootAdmission {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RootAdmissionState {
                id: NEXT_ROOT_ADMISSION_ID.fetch_add(1, Ordering::Relaxed),
                next_chain_id: AtomicU64::new(1),
                graph: StdMutex::new(WaitGraph::default()),
            }),
        }
    }

    pub fn root_context(&self) -> ReentryContext {
        ReentryContext {
            admission_id: self.state.id,
            chain_id: self.state.next_chain_id.fetch_add(1, Ordering::Relaxed),
            depth: 0,
        }
    }

    pub fn admits(&self, context: ReentryContext) -> bool {
        context.admission_id == self.state.id
    }

    /// Starts a new causal chain and waits until it holds `gate`.
    ///
    /// A new chain holds no other Store, so its wait cannot be part of a cycle.
    pub async fn acquire_root(&self, gate: &StoreGate) -> wasmtime::Result<StoreAdmission> {
        let context = self.root_context();
        let waiting_since = Instant::now();
        tracing::trace!(
            wasm_plugin_admission_id = context.admission_id,
            wasm_plugin_chain_id = context.chain_id,
            wasm_plugin_store_gate_id = gate.id,
            "Waiting for legacy Wasm plugin root admission"
        );

        let permit = Arc::clone(&gate.permits)
            .acquire_owned()
            .await
            .map_err(|_| admission_closed())?;
        self.state.graph().hold(gate.id, context.chain_id);
        Ok(self.admitted(gate, context, permit, waiting_since))
    }

    /// Lets the chain of `context` into `gate`, waiting while another chain holds it.
    ///
    /// Stops waiting with [`ChainAdmission::Cycle`] once the holder waits on this chain.
    pub async fn admit_chain(
        &self,
        context: ReentryContext,
        gate: &StoreGate,
    ) -> wasmtime::Result<ChainAdmission> {
        let waiting_since = Instant::now();
        let wait_id = {
            let mut graph = self.state.graph();
            if graph.holder(gate.id) == Some(context.chain_id) {
                return Ok(ChainAdmission::Held);
            }
            if let Some(holder_chain_id) = graph.cycle_holder(context.chain_id, gate.id) {
                return Ok(ChainAdmission::Cycle { holder_chain_id });
            }
            graph.add_wait(context.chain_id, gate.id)
        };
        let _wait = WaitRegistration {
            state: Arc::clone(&self.state),
            wait_id,
        };
        tracing::trace!(
            wasm_plugin_admission_id = context.admission_id,
            wasm_plugin_chain_id = context.chain_id,
            wasm_plugin_store_gate_id = gate.id,
            "Waiting for legacy Wasm plugin store admission"
        );

        let acquire = Arc::clone(&gate.permits).acquire_owned();
        tokio::pin!(acquire);
        loop {
            // Keeps the same acquire future across re-checks, so the chain keeps its place in line.
            let attempt = tokio::time::timeout(WAIT_CYCLE_RECHECK_INTERVAL, acquire.as_mut());
            if let Ok(permit) = attempt.await {
                let permit = permit.map_err(|_| admission_closed())?;
                let mut graph = self.state.graph();
                graph.remove_wait(wait_id);
                graph.hold(gate.id, context.chain_id);
                drop(graph);
                let admission = self.admitted(gate, context, permit, waiting_since);
                return Ok(ChainAdmission::Admitted(admission));
            }
            let cycle = self.state.graph().cycle_holder(context.chain_id, gate.id);
            if let Some(holder_chain_id) = cycle {
                return Ok(ChainAdmission::Cycle { holder_chain_id });
            }
        }
    }

    fn admitted(
        &self,
        gate: &StoreGate,
        context: ReentryContext,
        permit: OwnedSemaphorePermit,
        waiting_since: Instant,
    ) -> StoreAdmission {
        let acquired_at = Instant::now();
        tracing::trace!(
            wasm_plugin_admission_id = context.admission_id,
            wasm_plugin_chain_id = context.chain_id,
            wasm_plugin_store_gate_id = gate.id,
            wasm_plugin_admission_wait_micros =
                acquired_at.duration_since(waiting_since).as_micros(),
            "Acquired legacy Wasm plugin store admission"
        );
        StoreAdmission {
            state: Arc::clone(&self.state),
            gate_id: gate.id,
            context,
            acquired_at,
            permit: Some(permit),
        }
    }
}

impl StoreAdmission {
    pub const fn context(&self) -> ReentryContext {
        self.context
    }
}

impl Drop for StoreAdmission {
    fn drop(&mut self) {
        tracing::trace!(
            wasm_plugin_admission_id = self.context.admission_id,
            wasm_plugin_chain_id = self.context.chain_id,
            wasm_plugin_store_gate_id = self.gate_id,
            wasm_plugin_admission_held_micros = self.acquired_at.elapsed().as_micros(),
            "Releasing legacy Wasm plugin store admission"
        );
        // Forget the holder before the next chain can take the gate, so the graph never shows a
        // chain holding a Store it already left.
        self.state
            .graph()
            .release(self.gate_id, self.context.chain_id);
        drop(self.permit.take());
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReentryContext {
    pub admission_id: u64,
    pub chain_id: u64,
    pub depth: usize,
}

impl ReentryContext {
    pub fn current() -> Option<Self> {
        REENTRY_CONTEXT.try_with(|context| *context).ok()
    }

    pub fn child(self) -> wasmtime::Result<Self> {
        if self.depth >= MAX_SYNC_REENTRY_DEPTH {
            return Err(wasmtime::Error::msg(format!(
                "Wasm plugin synchronous reentry exceeded the maximum depth of {MAX_SYNC_REENTRY_DEPTH}"
            )));
        }

        Ok(Self {
            admission_id: self.admission_id,
            chain_id: self.chain_id,
            depth: self.depth + 1,
        })
    }
}

tokio::task_local! {
    static REENTRY_CONTEXT: ReentryContext;
}

pub async fn scope<T>(context: ReentryContext, future: impl Future<Output = T>) -> T {
    REENTRY_CONTEXT.scope(context, future).await
}

pub fn sync_scope<T>(context: ReentryContext, operation: impl FnOnce() -> T) -> T {
    REENTRY_CONTEXT.sync_scope(context, operation)
}

#[cfg(test)]
mod tests {
    use super::WaitGraph;

    #[test]
    fn wait_graph_reports_a_holder_that_waits_on_the_caller() {
        let mut graph = WaitGraph::default();
        graph.hold(10, 1);
        graph.hold(20, 2);
        assert_eq!(graph.cycle_holder(1, 20), None, "chain 2 waits on nothing");

        let wait = graph.add_wait(2, 10);
        assert_eq!(graph.cycle_holder(1, 20), Some(2));
        assert_eq!(
            graph.cycle_holder(3, 20),
            None,
            "a chain holding nothing can always wait"
        );

        graph.remove_wait(wait);
        assert_eq!(graph.cycle_holder(1, 20), None);
    }

    #[test]
    fn wait_graph_follows_cycles_through_several_chains() {
        let mut graph = WaitGraph::default();
        graph.hold(10, 1);
        graph.hold(20, 2);
        graph.hold(30, 3);
        graph.add_wait(2, 30);
        graph.add_wait(3, 10);
        assert_eq!(graph.cycle_holder(1, 20), Some(2));

        graph.release(10, 1);
        assert_eq!(graph.cycle_holder(1, 20), None);
        graph.release(20, 1);
        assert_eq!(graph.holder(20), Some(2), "only the holder releases a gate");
    }
}
