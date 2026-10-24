//! Block states that plugins asked the server to report to them.

use std::sync::{
    Mutex,
    atomic::{AtomicU8, Ordering},
};

use pumpkin_data::BlockStateId;
use rustc_hash::FxHashMap;

/// What a plugin hooks a block state for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockStateHook {
    /// Fire `BlockShapeUpdateEvent` before a shape update changes the state.
    ShapeUpdate = 1 << 0,
    /// Fire `BlockScheduledTickEvent` before a scheduled tick of the state runs.
    ScheduledTick = 1 << 1,
}

/// The states each plugin registered, per hook.
type Registrations = FxHashMap<String, FxHashMap<BlockStateHook, Vec<BlockStateId>>>;

impl BlockStateHook {
    const fn bit(self) -> u8 {
        self as u8
    }
}

/// Per-state flags for the hooks plugins registered.
///
/// Lookups are relaxed atomic loads, so they can run for every block update on any thread.
/// Changes take a lock and rebuild the flags from the sets of all plugins.
pub struct BlockStateHooks {
    flags: Box<[AtomicU8]>,
    /// Union of all flags, so hot paths skip the table while nothing is registered.
    active: AtomicU8,
    registrations: Mutex<Registrations>,
}

impl Default for BlockStateHooks {
    fn default() -> Self {
        Self {
            flags: (0..BlockStateId::COUNT).map(|_| AtomicU8::new(0)).collect(),
            active: AtomicU8::new(0),
            registrations: Mutex::new(FxHashMap::default()),
        }
    }
}

impl BlockStateHooks {
    /// Returns whether any plugin registered states for `hook`.
    #[inline]
    #[must_use]
    pub fn is_active(&self, hook: BlockStateHook) -> bool {
        self.active.load(Ordering::Relaxed) & hook.bit() != 0
    }

    /// Returns whether any plugin registered `state` for `hook`.
    #[inline]
    #[must_use]
    pub fn contains(&self, hook: BlockStateHook, state: BlockStateId) -> bool {
        self.is_active(hook)
            && self.flags[state.as_u16() as usize].load(Ordering::Relaxed) & hook.bit() != 0
    }

    /// Replaces the states `plugin` registered for `hook`. An empty list removes them.
    pub fn set(&self, plugin: &str, hook: BlockStateHook, states: Box<[BlockStateId]>) {
        let mut registrations = self
            .registrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if states.is_empty() {
            if let Some(hooks) = registrations.get_mut(plugin) {
                hooks.remove(&hook);
                if hooks.is_empty() {
                    registrations.remove(plugin);
                }
            }
        } else {
            registrations
                .entry(plugin.to_string())
                .or_default()
                .insert(hook, states.into_vec());
        }
        self.rebuild(&registrations);
    }

    /// Adds `states` to the states `plugin` registered for `hook`.
    pub fn add(
        &self,
        plugin: &str,
        hook: BlockStateHook,
        states: impl IntoIterator<Item = BlockStateId>,
    ) {
        let mut registrations = self
            .registrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registered = registrations
            .entry(plugin.to_string())
            .or_default()
            .entry(hook)
            .or_default();
        registered.extend(states);
        registered.sort_unstable();
        registered.dedup();
        self.rebuild(&registrations);
    }

    /// Removes every state `plugin` registered.
    pub fn remove_plugin(&self, plugin: &str) {
        let mut registrations = self
            .registrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registrations.remove(plugin).is_some() {
            self.rebuild(&registrations);
        }
    }

    fn rebuild(&self, registrations: &Registrations) {
        let mut flags = vec![0u8; self.flags.len()];
        let mut active = 0;
        for (hook, states) in registrations.values().flatten() {
            for state in states {
                flags[state.as_u16() as usize] |= hook.bit();
            }
            active |= hook.bit();
        }
        // Store each entry directly so other plugins' hooks never read as unset meanwhile.
        for (slot, value) in self.flags.iter().zip(flags) {
            slot.store(value, Ordering::Relaxed);
        }
        self.active.store(active, Ordering::Relaxed);
    }
}
