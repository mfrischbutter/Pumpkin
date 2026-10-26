use std::future::Future;

use crate::chain::{
    ChainAdmission, ReentryContext, RootAdmission, StoreAdmission, StoreGate, scope,
};

mod sealed {
    pub trait StorePolicy {}
}

#[doc(hidden)]
pub trait StorePolicy: sealed::StorePolicy + Clone + Send + Sync + 'static {
    const NAME: &'static str;
}

/// Admits one synchronous causal chain at a time into each Store while allowing callbacks from
/// the active chain to re-enter Stores already on that chain.
///
/// Stores do not wait for each other: a busy Store only holds back the chains that call into it.
/// A chain that would wait for a Store whose holder waits on that chain joins the Store's active
/// reentry scope instead, so opposing calls between two plugins cannot deadlock.
#[derive(Clone, Debug)]
pub struct LegacySyncReentry {
    root_admission: RootAdmission,
}

impl LegacySyncReentry {
    pub const NAME: &'static str = "LegacySyncReentry";

    /// Creates one admission authority. Clone and share this policy across all
    /// Stores that can participate in the same synchronous plugin call graph, so
    /// they share causal chain ids and see each other's waits.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root_admission: RootAdmission::new(),
        }
    }

    #[must_use]
    pub(crate) fn inherited_context(&self) -> Option<ReentryContext> {
        ReentryContext::current().filter(|context| self.root_admission.admits(*context))
    }

    pub(crate) async fn acquire_root(&self, gate: &StoreGate) -> wasmtime::Result<StoreAdmission> {
        self.root_admission.acquire_root(gate).await
    }

    pub(crate) async fn admit_chain(
        &self,
        context: ReentryContext,
        gate: &StoreGate,
    ) -> wasmtime::Result<ChainAdmission> {
        self.root_admission.admit_chain(context, gate).await
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn root_context(&self) -> ReentryContext {
        self.root_admission.root_context()
    }

    /// Runs Store bootstrap work in a causal chain of this admission authority, so
    /// callbacks it causes in other Stores are admitted like those of later guest calls.
    ///
    /// The Store being set up has no gate yet and nothing can call into it, so the
    /// bootstrap chain holds no Store and only waits for the Stores it calls into.
    pub async fn scope_bootstrap<T>(
        &self,
        future: impl Future<Output = wasmtime::Result<T>>,
    ) -> wasmtime::Result<T> {
        if let Some(context) = self.inherited_context() {
            tracing::trace!(
                wasm_plugin_admission_id = context.admission_id,
                wasm_plugin_chain_id = context.chain_id,
                wasm_plugin_reentry_depth = context.depth,
                "Inherited legacy Wasm plugin causal chain"
            );
            return scope(context, future).await;
        }

        let context = self.root_admission.root_context();
        tracing::trace!(
            wasm_plugin_admission_id = context.admission_id,
            wasm_plugin_chain_id = context.chain_id,
            "Started legacy Wasm plugin bootstrap chain"
        );
        scope(context, future).await
    }
}

impl Default for LegacySyncReentry {
    fn default() -> Self {
        Self::new()
    }
}

impl sealed::StorePolicy for LegacySyncReentry {}

impl StorePolicy for LegacySyncReentry {
    const NAME: &'static str = Self::NAME;
}
