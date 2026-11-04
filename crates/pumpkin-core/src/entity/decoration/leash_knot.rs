use crate::entity::decoration::block_attached::{
    BlockAttached, BlockAttachment, attached_ignore_explosion, emit_game_event, send_spawn_at_block,
};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::net::java::JavaClient;
use crate::server::Server;
use crate::world::World;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

/// Vanilla `Leashable.leashableInArea`: the side of the cube searched for leashed entities.
const LEASHABLE_AREA_SIZE: f64 = 32.0;

/// Vanilla `Leashable.leashableInArea` around `center`, with the test
/// `getLeashHolder() == holder`.
pub(crate) fn leashed_to_holder(
    world: &World,
    center: Vector3<f64>,
    holder_id: i32,
) -> Vec<Arc<dyn EntityBase>> {
    let area = BoundingBox::of_size(
        center,
        LEASHABLE_AREA_SIZE,
        LEASHABLE_AREA_SIZE,
        LEASHABLE_AREA_SIZE,
    );
    world
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|entity| {
            entity
                .get_entity()
                .leashed_to
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|holder| holder.get_entity().entity_id == holder_id)
        })
        .collect()
}

/// Vanilla `Leashable.canHaveALeashAttachedTo`, whose `canBeLeashed` the entities in question
/// already passed when they were leashed.
pub(crate) fn can_have_leash_attached_to(leashable: &Entity, holder: &Entity) -> bool {
    leashable.entity_id != holder.entity_id
        && leashable.leash_distance_to(holder) <= Entity::LEASH_SNAP_DISTANCE
}

/// The center of an entity's box, vanilla `getBoundingBox().getCenter()`.
pub(crate) fn box_center(entity: &Entity) -> Vector3<f64> {
    let bounds = entity.bounding_box.load();
    Vector3::new(
        f64::midpoint(bounds.min.x, bounds.max.x),
        f64::midpoint(bounds.min.y, bounds.max.y),
        f64::midpoint(bounds.min.z, bounds.max.z),
    )
}

/// Vanilla `LeashFenceKnotEntity`.
pub struct LeashKnotEntity {
    entity: Entity,
    attachment: BlockAttachment,
}

impl LeashKnotEntity {
    pub const OFFSET_Y: f64 = 0.375;

    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let pos = entity.block_pos.load();
        Self {
            entity,
            attachment: BlockAttachment::new(pos),
        }
    }

    /// Vanilla `getOrCreateKnot`.
    pub fn get_or_create(world: &Arc<World>, pos: BlockPos) -> Arc<dyn EntityBase> {
        Self::get_knot(world, pos).unwrap_or_else(|| Self::create_knot(world, pos))
    }

    /// Vanilla `getKnot`.
    pub fn get_knot(world: &Arc<World>, pos: BlockPos) -> Option<Arc<dyn EntityBase>> {
        let (x, y, z) = (f64::from(pos.0.x), f64::from(pos.0.y), f64::from(pos.0.z));
        let search_box = BoundingBox::new(
            Vector3::new(x - 1.0, y - 1.0, z - 1.0),
            Vector3::new(x + 1.0, y + 1.0, z + 1.0),
        );
        world
            .get_entities_at_box(&search_box)
            .into_iter()
            .find(|entity| {
                entity
                    .cast_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|knot| knot.attachment.pos() == pos)
            })
    }

    /// Vanilla `createKnot`, which plays no sound of its own.
    pub fn create_knot(world: &Arc<World>, pos: BlockPos) -> Arc<dyn EntityBase> {
        let raw_pos = Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + Self::OFFSET_Y,
            f64::from(pos.0.z) + 0.5,
        );

        let entity = Entity::new(world.clone(), raw_pos, &EntityType::LEASH_KNOT);
        let knot: Arc<dyn EntityBase> = Arc::new(Self::new(entity));
        world.spawn_entity(knot.clone());
        knot
    }

    /// The part of vanilla `Entity.dropAllLeashConnections` and `shearOffAllLeashConnections`
    /// a knot needs: every leash on it comes off with its lead.
    fn shear_off_leashes(&self, player: &Player) -> bool {
        let world = self.entity.world.load();
        let leashed = leashed_to_holder(&world, box_center(&self.entity), self.entity.entity_id);
        if leashed.is_empty() {
            return false;
        }
        for mob in &leashed {
            mob.get_entity().drop_leash();
        }
        emit_game_event(&self.entity, GameEvent::Shear, Some(player));
        world.play_sound(
            Sound::ItemShearsSnip,
            SoundCategory::Players,
            &self.attachment.pos().to_centered_f64(),
        );
        true
    }

    /// The loop of vanilla `interact` that ties the player's mobs to this knot.
    fn attach_player_mobs(&self, world: &World, player: &Player, player_id: i32) -> bool {
        let Some(knot) = world.get_entity_by_id(self.entity.entity_id) else {
            return false;
        };
        let mut attached_mob = false;
        for mob in leashed_to_holder(world, box_center(player.get_entity()), player_id) {
            if can_have_leash_attached_to(mob.get_entity(), &self.entity) {
                mob.get_entity().leash_to(knot.clone());
                attached_mob = true;
            }
        }
        attached_mob
    }

    /// The loop of vanilla `interact` that hands this knot's mobs to the player.
    fn hand_mobs_to_player(&self, world: &World, player: &Arc<Player>) -> bool {
        let mut any_dropped = false;
        let holder: Arc<dyn EntityBase> = player.clone();
        for mob in leashed_to_holder(world, box_center(&self.entity), self.entity.entity_id) {
            if can_have_leash_attached_to(mob.get_entity(), holder.get_entity()) {
                mob.get_entity().leash_to(holder.clone());
                any_dropped = true;
            }
        }
        any_dropped
    }
}

impl BlockAttached for LeashKnotEntity {
    fn attachment(&self) -> &BlockAttachment {
        &self.attachment
    }

    /// Vanilla `LeashFenceKnotEntity.survives`.
    fn survives(&self) -> bool {
        self.entity
            .world
            .load()
            .get_block(&self.attachment.pos())
            .has_tag(&pumpkin_data::tag::Block::MINECRAFT_FENCES)
    }

    /// Vanilla `LeashFenceKnotEntity.dropItem`: only a sound, each leashed mob drops its own lead.
    fn drop_attached_item(&self, _caused_by: Option<&dyn EntityBase>) {
        self.play_entity_sound(Sound::ItemLeadUntied, 1.0, 1.0);
    }
}

impl EntityBase for LeashKnotEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.tick_attached(caller);
    }

    fn send_java_spawn_packet(&self, client: &JavaClient) {
        send_spawn_at_block(self, self.attachment.pos(), client);
    }

    /// Vanilla `LeashFenceKnotEntity.interact`.
    fn interact_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        if item_stack.item.id == Item::SHEARS.id && self.shear_off_leashes(player) {
            let _ = player.damage_item_stack(item_stack, 1);
            return Some(true);
        }

        let world = self.entity.world.load();
        let attached_mob = self.attach_player_mobs(&world, player, player.entity_id());
        let any_dropped = !attached_mob
            && !player.get_entity().is_sneaking()
            && self.hand_mobs_to_player(&world, player);

        if attached_mob || any_dropped {
            emit_game_event(&self.entity, GameEvent::BlockAttach, Some(&**player));
            self.play_entity_sound(Sound::ItemLeadTied, 1.0, 1.0);
            Some(true)
        } else {
            Some(false)
        }
    }

    /// Vanilla `LeashFenceKnotEntity.notifyLeasheeRemoved`.
    fn notify_leashee_removed(&self) {
        let world = self.entity.world.load();
        if leashed_to_holder(&world, box_center(&self.entity), self.entity.entity_id).is_empty() {
            self.entity.remove();
        }
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.hurt_attached(damage_type, cause)
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.kill_attached(None);
    }

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn move_entity(&self, _caller: &dyn EntityBase, motion: Vector3<f64>) {
        self.moved_off_block(motion);
    }

    fn push_from_explosion(&self, impulse: Vector3<f64>) {
        self.moved_off_block(impulse);
    }

    fn ignore_explosion(&self, affects_blocklike_entities: bool, source_in_water: bool) -> bool {
        attached_ignore_explosion(affects_blocklike_entities, source_in_water)
    }

    fn skip_attack_interaction(&self, player: &Player) -> bool {
        self.skip_attack_on_attached(player)
    }

    /// Vanilla `BlockAttachedEntity.thunderHit` does nothing.
    fn on_lightning_strike(
        &self,
        _caller: &dyn EntityBase,
        _lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
