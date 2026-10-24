use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::{
    LoggerOption,
    block::blocks::redstone::SIGNAL_MAX,
    command::client_suggestions,
    data::datapack::JUKEBOX_SONG_REGISTRY,
    net::ClientPlatform,
    plugin::{PluginMetadata, block_hooks::BlockStateHook},
    data::datapack::{ENCHANTMENT_REGISTRY, JUKEBOX_SONG_REGISTRY},
    net::ClientPlatform,
    plugin::PluginMetadata,
    plugin_log,
};
use arc_swap::ArcSwap;
use pumpkin_data::jukebox_song_registry::{
    DynamicJukeboxSong, register_dynamic_jukebox_song, unregister_dynamic_jukebox_song,
};
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::NbtCompound;
use pumpkin_util::{
    PermissionLvl,
    identifier::Identifier,
    permission::{Permission, PermissionManager},
    text::TextComponent,
};
use tracing::{Level, warn};

use crate::{
    entity::player::Player,
    plugin::{EventHandler, HandlerMap, PluginManager, TypedEventHandler, insert_handler},
    server::Server,
};

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

use super::{EventPriority, Payload};

/// The `Context` struct represents the context of a plugin, containing metadata,
/// a server reference, and event handlers.
///
/// # Fields
/// - `metadata`: Metadata of the plugin.
/// - `server`: A reference to the server on which the plugin operates.
/// - `handlers`: A map of event handlers, wrapped in `ArcSwap` for lock-free read access across threads.
#[repr(C)]
pub struct Context {
    metadata: PluginMetadata,
    pub server: Arc<Server>,
    pub handlers: Arc<ArcSwap<HandlerMap>>,
    pub plugin_manager: Arc<PluginManager>,
    pub permission_manager: Arc<PermissionManager>,
    pub logger: Arc<OnceLock<LoggerOption>>,
}
impl Context {
    /// Creates a new instance of `Context`.
    ///
    /// # Arguments
    /// - `metadata`: The metadata of the plugin.
    /// - `server`: A reference to the server.
    /// - `handlers`: A collection containing the event handlers.
    ///
    /// # Returns
    /// A new instance of `Context`.
    #[must_use]
    pub fn new(
        metadata: PluginMetadata,
        server: Arc<Server>,
        handlers: Arc<ArcSwap<HandlerMap>>,
        plugin_manager: Arc<PluginManager>,
        logger: Arc<OnceLock<LoggerOption>>,
    ) -> Self {
        let permission_manager = server.permission_manager.clone();
        Self {
            metadata,
            server,
            handlers,
            plugin_manager,
            permission_manager,
            logger,
        }
    }

    #[must_use]
    pub const fn get_metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    /// Retrieves the data folder path for the plugin, creating it if it does not exist.
    ///
    /// # Returns
    /// A string representing the path to the data folder.
    #[must_use]
    pub fn get_data_folder(&self) -> PathBuf {
        let path = Path::new("plugins").join("data").join(&self.metadata.name);
        if !path.exists() {
            let _ = fs::create_dir_all(&path);
        }
        path
    }

    /// Asynchronously retrieves a player by their name.
    ///
    /// # Arguments
    /// - `player_name`: The name of the player to retrieve.
    ///
    /// # Returns
    /// An optional reference to the player if found, or `None` if not.
    #[must_use]
    pub fn get_player_by_name(&self, player_name: &str) -> Option<Arc<Player>> {
        self.server.get_player_by_name(player_name)
    }

    /// Registers a service with the plugin context.
    ///
    /// This method allows you to associate a service instance with a given name,
    /// making it available for retrieval by plugins or other components.
    /// The service must be wrapped in an `Arc` and implement `Payload`.
    ///
    /// # Arguments
    ///
    /// * `name` - The unique name to register the service under.
    /// * `service` - The service instance to register, wrapped in an `Arc`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// context.register_service("my_service", Arc::new(MyService::new())).await;
    /// ```
    pub async fn register_service<N: Into<String>, T: Payload + 'static>(
        &self,
        name: N,
        service: Arc<T>,
    ) {
        let mut services = self.plugin_manager.services.write().await;
        services.insert(name.into(), service);
    }

    /// Retrieves a registered service by name and type.
    ///
    /// This method attempts to fetch a service previously registered under the given name,
    /// and downcasts it to the requested type using name-based type checking.
    /// Returns `Some(Arc<T>)` if the service exists and the type matches, or `None` otherwise.
    ///
    /// This method is safe to use across compilation boundaries as it uses string-based
    /// type identification instead of `TypeId`.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the service to retrieve.
    ///
    /// # Returns
    ///
    /// An `Option<Arc<T>>` containing the service if found and type matches, or `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(service) = context.get_service::<MyService>("my_service").await {
    ///     // Use the service
    /// }
    /// ```
    pub async fn get_service<T: Payload + 'static>(&self, name: &str) -> Option<Arc<T>> {
        let services = self.plugin_manager.services.read().await;
        let service = services.get(name)?.clone();
        <dyn Payload>::downcast_arc::<T>(service)
    }

    /// Registers a new command to the server with a specified permission level.
    ///
    /// # Arguments
    /// - `node`: The command node to register.
    /// - `permission`: The permission level required to execute the command.
    pub fn register_command<P: Into<String>>(
        &self,
        node: impl Into<crate::command::node::detached::CommandDetachedNode>,
        permission: P,
    ) {
        let permission = permission.into();

        let full_permission_node = if permission.contains(':') {
            permission
        } else {
            format!("{}:{permission}", self.metadata.name)
        };

        let mut node = node.into();
        node.meta.source = Some(self.metadata.name.clone());
        node.owned
            .requirements
            .0
            .push(crate::command::node::Requirement(Arc::new(move |source| {
                source.has_permission(&full_permission_node)
            })));

        self.server.command_dispatcher.rcu(|dispatcher| {
            let mut new_dispatcher = (**dispatcher).clone();
            new_dispatcher.register(node.clone());
            Arc::new(new_dispatcher)
        });

        self.reload_commands_for_everyone();
    }

    /// Registers a new command with aliases to the server with a specified permission level.
    pub fn register_command_with_aliases<P: Into<String>>(
        &self,
        node: impl Into<crate::command::node::detached::CommandDetachedNode>,
        aliases: &[String],
        permission: P,
    ) {
        let permission = permission.into();

        let full_permission_node = if permission.contains(':') {
            permission
        } else {
            format!("{}:{permission}", self.metadata.name)
        };

        let mut node = node.into();
        node.meta.source = Some(self.metadata.name.clone());
        node.owned
            .requirements
            .0
            .push(crate::command::node::Requirement(Arc::new(move |source| {
                source.has_permission(&full_permission_node)
            })));

        self.server.command_dispatcher.rcu(|dispatcher| {
            let mut new_dispatcher = (**dispatcher).clone();
            new_dispatcher.register_with_aliases(node.clone(), aliases);
            Arc::new(new_dispatcher)
        });

        self.reload_commands_for_everyone();
    }

    /// Unregisters a command from the server.
    ///
    /// # Arguments
    /// - `name`: The name of the command to unregister.
    pub fn unregister_command(&self, name: &str) {
        self.server.command_dispatcher.rcu(|dispatcher| {
            let mut new_dispatcher = (**dispatcher).clone();
            new_dispatcher.deactivate_plugin_command_and_aliases(name);
            Arc::new(new_dispatcher)
        });

        self.reload_commands_for_everyone();
    }

    pub(crate) fn unregister_commands(&self) {
        let source = self.metadata.name.clone();
        self.server.command_dispatcher.rcu(|dispatcher| {
            let mut new_dispatcher = (**dispatcher).clone();
            new_dispatcher.deactivate_commands_from_source(&source);
            Arc::new(new_dispatcher)
        });

        self.reload_commands_for_everyone();
    }

    /// Fires `BlockShapeUpdateEvent` for shape updates that would change one of `states`.
    ///
    /// Replaces the states this plugin watched before; an empty slice stops watching.
    pub fn watch_block_states(&self, states: Box<[BlockStateId]>) {
        let hooks = &self.server.block_registry.plugin_hooks;
        hooks.set(&self.metadata.name, BlockStateHook::ShapeUpdate, states);
    }

    /// Fires `BlockRandomTickEvent` for random ticks of `states`. States vanilla never random
    /// ticks get random ticks for this, which only reach plugins.
    ///
    /// Replaces the states this plugin watched before; an empty slice stops watching.
    pub fn watch_random_tick_states(&self, states: Box<[BlockStateId]>) {
        let hooks = &self.server.block_registry.plugin_hooks;
        if hooks.set(&self.metadata.name, BlockStateHook::RandomTick, states) {
            self.rebuild_random_tick_caches();
        }
    }

    /// Fires `BlockScheduledTickEvent` before each scheduled tick of `block` runs.
    pub fn register_scheduled_tick_hook(&self, block: &Block) {
        let states = block.states.iter().map(|state| state.id);
        let hooks = &self.server.block_registry.plugin_hooks;
        hooks.add(&self.metadata.name, BlockStateHook::ScheduledTick, states);
    }

    pub(crate) fn unregister_block_hooks(&self) {
        let hooks = &self.server.block_registry.plugin_hooks;
        if hooks.remove_plugin(&self.metadata.name) {
            self.rebuild_random_tick_caches();
        }
    }

    /// Recounts the random-ticking blocks of loaded chunks after the plugin random-ticking
    /// states changed. Runs on Rayon, as it walks every loaded section.
    fn rebuild_random_tick_caches(&self) {
        for world in self.server.worlds.load().iter() {
            let level = world.level.clone();
            rayon::spawn(move || level.rebuild_random_tick_caches());
        }
    }

    /// Reloads (resends) all commands for all currently online players.
    pub fn reload_commands_for_everyone(&self) {
        for world in self.server.worlds.load().iter() {
            for player in world.players.load().iter() {
                self.reload_commands_for(player);
            }
        }
    }

    /// Reloads (resends) all commands for a particular player on the server.
    ///
    /// # Arguments
    /// - `player`: The player for which the commands will be reloaded.
    pub fn reload_commands_for(&self, player: &Arc<Player>) {
        let command_dispatcher = self.server.command_dispatcher.load();
        if let ClientPlatform::Bedrock(_) = player.client.as_ref() {
            client_suggestions::send_bedrock_commands_packet(
                player,
                &self.server,
                &command_dispatcher,
            );
        } else {
            client_suggestions::send_c_commands_packet(player, &self.server, &command_dispatcher);
        }
    }

    /// Register a permission for this plugin
    pub fn register_permission(&self, permission: Permission) -> Result<(), String> {
        // Ensure the permission has the correct namespace
        if !permission
            .node
            .starts_with(&format!("{}:", self.metadata.name))
        {
            return Err(format!(
                "Permission {} must use the plugin's namespace ({})",
                permission.node, self.metadata.name
            ));
        }

        self.permission_manager.register_permission(permission)
    }

    /// Registers a loot table from its JSON definition, like a datapack's
    /// `data/<namespace>/loot_table/<path>.json`. It replaces a vanilla table with the same id,
    /// while a datapack table with the same id still wins. It stays registered across `/reload`
    /// until the plugin unloads.
    pub fn register_loot_table(&self, id: &str, json: &str) -> Result<(), String> {
        let table = crate::data::datapack::loot_table_loader::parse_loot_table(json)
            .ok_or_else(|| format!("Loot table {id} is not valid loot table JSON"))?;
        self.server.datapack_manager.insert_plugin_loot_table(
            &self.metadata.name,
            id,
            Arc::new(table),
        );
        Ok(())
    }

    pub(crate) fn unregister_loot_tables(&self) {
        self.server
            .datapack_manager
            .remove_plugin_loot_tables(&self.metadata.name);
    }

    /// Adds an entry to a synced registry, such as a chat type or a wolf variant.
    ///
    /// `data` is the entry in the registry's NBT form, as a datapack defines it. Players receive
    /// the entry from their next configuration on. It is removed when this plugin unloads.
    pub fn register_registry_entry(
        &self,
        registry: &str,
        id: &str,
        data: NbtCompound,
    ) -> Result<(), String> {
        self.server.datapack_manager.register_plugin_registry_entry(
            &self.metadata.name,
            registry,
            id,
            data,
        )?;
        self.warn_if_players_online(id);
        Ok(())
    }

    /// Registers a jukebox song. Jukeboxes play discs whose `jukebox_playable` names `id`, with
    /// this length and comparator signal, and clients show `description` as "now playing" and
    /// in the disc's tooltip. The song is removed when this plugin unloads.
    pub fn register_jukebox_song(
        &self,
        id: &str,
        sound_event: &str,
        description: &TextComponent,
        length_in_seconds: f32,
        comparator_output: u8,
    ) -> Result<(), String> {
        let sound_event = Identifier::parse(sound_event).map_err(|error| error.to_string())?;
        // Vanilla's `JukeboxSong` codec takes a positive length and a signal from 0 to 15
        // (from memory).
        if !length_in_seconds.is_finite() || length_in_seconds <= 0.0 {
            return Err(format!("Jukebox song length must be positive, got {length_in_seconds}"));
        }
        if comparator_output > SIGNAL_MAX {
            return Err(format!(
                "Jukebox song comparator output must be at most {SIGNAL_MAX}, got {comparator_output}"
            ));
        }
        let mut data = NbtCompound::new();
        data.put_string("sound_event", sound_event.to_string());
        data.put("description", description.to_nbt_tag_for_version(&CURRENT_MC_VERSION));
        data.put_float("length_in_seconds", length_in_seconds);
        data.put_int("comparator_output", i32::from(comparator_output));

        let id = self.server.datapack_manager.insert_plugin_registry_entry(
            &self.metadata.name,
            JUKEBOX_SONG_REGISTRY.to_string(),
            id,
            data,
        )?;
        register_dynamic_jukebox_song(
            id.clone(),
            DynamicJukeboxSong {
                length_in_seconds,
                comparator_output,
            },
        );
        self.warn_if_players_online(&id);
        Ok(())
    }

    /// Players that already configured keep the registries they received.
    fn warn_if_players_online(&self, entry_id: &str) {
        if self.server.get_player_count() > 0 {
            warn!(
                "Registry entry {entry_id} of plugin {} was added while players are online; they receive it when they rejoin",
                self.metadata.name
            );
        }
    }

    /// Removes the registry entries this plugin added, and the server's model of them.
    pub(crate) async fn unregister_registry_entries(&self) {
        let removed = self
            .server
            .datapack_manager
            .remove_plugin_registry_entries(&self.metadata.name);
        for (registry_id, entry_id) in removed {
            if registry_id == JUKEBOX_SONG_REGISTRY {
                unregister_dynamic_jukebox_song(&entry_id);
            } else if registry_id == ENCHANTMENT_REGISTRY {
                self.server.enchantment_manager.unregister(&entry_id).await;
            }
        }
    }

    /// Check if a player has a permission
    #[must_use]
    pub fn player_has_permission(&self, player_uuid: &uuid::Uuid, permission: &str) -> bool {
        // If the player isn't online, we need to find their op level
        let player_op_level = self
            .server
            .get_player_by_uuid(*player_uuid)
            .map_or(PermissionLvl::Zero, |player| player.permission_lvl.load());

        self.permission_manager
            .has_permission(player_uuid, permission, player_op_level)
    }

    /// Registers an event handler for a specific event type.
    ///
    /// # Type Parameters
    /// - `E`: The event type that the handler will respond to.
    /// - `H`: The type of the event handler.
    ///
    /// # Arguments
    /// - `handler`: A reference to the event handler.
    /// - `priority`: The priority of the event handler.
    /// - `blocking`: A boolean indicating whether the handler is blocking.
    ///
    /// # Constraints
    /// The handler must implement the `EventHandler<E>` trait.
    pub fn register_event<E: Payload + 'static, H>(
        &self,
        handler: Arc<H>,
        priority: EventPriority,
        blocking: bool,
    ) where
        H: EventHandler<E> + 'static,
    {
        let typed_handler = Arc::new(TypedEventHandler {
            handler,
            priority,
            blocking,
            source: Some(self.metadata.name.clone()),
            _phantom: std::marker::PhantomData,
        });

        self.handlers.rcu(|handlers| {
            let mut new_handlers = (**handlers).clone();
            insert_handler(
                new_handlers.entry(E::get_name_static()).or_default(),
                typed_handler.clone(),
            );
            Arc::new(new_handlers)
        });
    }

    /// Registers a custom plugin loader that can load additional plugin types.
    ///
    /// This method allows plugins to extend the server with support for loading
    /// plugins in different formats (e.g., Lua, JavaScript, Python). When a new
    /// loader is registered, the plugin manager will automatically attempt to load
    /// any previously unloadable files in the plugins directory with this new loader.
    ///
    /// # Arguments
    /// - `loader`: The custom plugin loader implementation to register.
    ///
    /// # Returns
    /// `true` if new plugins were loaded as a result of registering this loader, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Create and register a custom Lua plugin loader
    /// let lua_loader = Arc::new(LuaPluginLoader::new());
    /// context.register_plugin_loader(lua_loader).await;
    /// ```
    pub async fn register_plugin_loader(
        &self,
        loader: Arc<dyn crate::plugin::loader::PluginLoader>,
    ) -> bool {
        let before_count = self.plugin_manager.loaded_plugins().len();
        self.plugin_manager.add_loader(&self.server, loader).await;
        let after_count = self.plugin_manager.loaded_plugins().len();

        // Return true if any new plugins were loaded
        after_count > before_count
    }

    /// Initializes logging via the tracing crate for the plugin.
    pub fn init_log(&self) {
        if let Some(Some((_logger_impl, level, config))) = self.logger.get() {
            let fmt_layer = fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(config.color)
                .with_ansi_sanitization(false)
                .with_target(config.target)
                .with_thread_names(config.threads)
                .with_thread_ids(config.thread_ids);

            if config.timestamp {
                let fmt_layer = fmt_layer.with_timer(fmt::time::UtcTime::new(
                    time::macros::format_description!(
                        "[year]-[month]-[day] [hour]:[minute]:[second]"
                    ),
                ));
                let _ = tracing_subscriber::registry()
                    .with(*level)
                    .with(fmt_layer)
                    .try_init();
            } else {
                let fmt_layer = fmt_layer.without_time();
                let _ = tracing_subscriber::registry()
                    .with(*level)
                    .with(fmt_layer)
                    .try_init();
            }
        }
    }

    pub fn log(&self, message: impl std::fmt::Display) {
        let level = if let Some(Some((_, level, _))) = self.logger.get() {
            level.into_level().unwrap_or(Level::INFO)
        } else {
            Level::INFO
        };
        plugin_log!(level, &self.metadata.name, "{}", message);
    }
}
