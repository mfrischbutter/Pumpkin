//! Template-based structure piece matching vanilla Minecraft `TemplateStructurePiece`.
//!
//! This module provides `TemplatePiece`, which implements `StructurePieceBase`
//! to place blocks from a loaded structure template into the world.

use std::sync::Arc;

use pumpkin_data::{Mirror, Rotation};
use pumpkin_util::{
    math::{block_box::BlockBox, vector3::Vector3},
    random::RandomGenerator,
};

use crate::{
    ProtoChunk,
    generation::{
        proto_chunk::GenerationCache,
        structure::{
            piece::StructurePieceType,
            structures::{StructurePiece, StructurePieceBase},
        },
    },
    world::WorldPortalExt,
};

use super::{RegionPlacer, StructurePlaceSettings, StructureTemplate, place_template_piece};

/// A structure piece that places blocks from an NBT template matching vanilla `TemplateStructurePiece`.
///
/// It has no data markers of its own.
pub struct TemplatePiece {
    /// The underlying structure piece metadata.
    pub piece: StructurePiece,

    /// The template identifier/name.
    pub template_name: String,

    /// The template to place.
    pub template: Arc<StructureTemplate>,

    /// Placement settings (rotation, mirror, pivot, processors, etc.).
    pub place_settings: StructurePlaceSettings,

    /// The origin position of the template in the world.
    pub template_position: Vector3<i32>,
}

impl TemplatePiece {
    /// Creates a new template piece from the given `template` at `origin` with legacy rotation & mirror.
    #[must_use]
    pub fn new(
        template: Arc<StructureTemplate>,
        rotation: Rotation,
        mirror: Mirror,
        origin: Vector3<i32>,
        piece_type: StructurePieceType,
    ) -> Self {
        let place_settings = StructurePlaceSettings::new()
            .set_rotation(rotation)
            .set_mirror(mirror);
        let bounding_box = template.get_bounding_box(&place_settings, origin);

        Self {
            piece: StructurePiece::new(piece_type, bounding_box, 0),
            template_name: String::new(),
            template,
            place_settings,
            template_position: origin,
        }
    }

    /// Creates a new template piece with full `StructurePlaceSettings` matching vanilla `TemplateStructurePiece`.
    #[must_use]
    pub fn from_settings(
        template: Arc<StructureTemplate>,
        template_name: String,
        place_settings: StructurePlaceSettings,
        origin: Vector3<i32>,
        piece_type: StructurePieceType,
    ) -> Self {
        let bounding_box = template.get_bounding_box(&place_settings, origin);

        Self {
            piece: StructurePiece::new(piece_type, bounding_box, 0),
            template_name,
            template,
            place_settings,
            template_position: origin,
        }
    }

    /// Creates a template piece with a specific chain length.
    #[must_use]
    pub fn with_chain_length(
        template: Arc<StructureTemplate>,
        rotation: Rotation,
        mirror: Mirror,
        origin: Vector3<i32>,
        piece_type: StructurePieceType,
        chain_length: u32,
    ) -> Self {
        let mut piece = Self::new(template, rotation, mirror, origin, piece_type);
        piece.piece.chain_length = chain_length;
        piece
    }

    /// Returns the template's size after applying rotation.
    #[must_use]
    pub fn rotated_size(&self) -> Vector3<i32> {
        self.place_settings
            .get_rotation()
            .transform_size(self.template.size)
    }

    /// Gets the rotation configured in place settings.
    #[must_use]
    pub const fn get_rotation(&self) -> Rotation {
        self.place_settings.get_rotation()
    }

    /// Gets a reference to the loaded template.
    #[must_use]
    pub const fn template(&self) -> &Arc<StructureTemplate> {
        &self.template
    }

    /// Gets the world template position.
    #[must_use]
    pub const fn template_position(&self) -> Vector3<i32> {
        self.template_position
    }

    /// Gets a reference to the structure placement settings.
    #[must_use]
    pub const fn place_settings(&self) -> &StructurePlaceSettings {
        &self.place_settings
    }

    /// Gets a mutable reference to the structure placement settings.
    pub const fn place_settings_mut(&mut self) -> &mut StructurePlaceSettings {
        &mut self.place_settings
    }

    /// Transforms a template-relative position to world coordinates.
    #[must_use]
    pub fn transform_pos(&self, local_pos: Vector3<i32>) -> Vector3<i32> {
        StructureTemplate::transform_block_pos(
            local_pos,
            self.place_settings.get_mirror(),
            self.place_settings.get_rotation(),
            self.place_settings.get_rotation_pivot(),
        ) + self.template_position
    }

    /// Moves the template position and bounding box by `(dx, dy, dz)`.
    pub fn move_piece(&mut self, dx: i32, dy: i32, dz: i32) {
        self.piece.translate(dx, dy, dz);
        self.template_position += Vector3::new(dx, dy, dz);
    }
}

impl StructurePieceBase for TemplatePiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        self.place_in_region(chunk, block_registry, random, seed, chunk_box);
    }

    fn place_in_region(
        &mut self,
        cache: &mut dyn GenerationCache,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        seed: i64,
        chunk_box: &BlockBox,
    ) {
        self.place_settings.bounding_box = Some(*chunk_box);
        self.piece.bounding_box = self
            .template
            .get_bounding_box(&self.place_settings, self.template_position);
        place_template_piece(
            &mut RegionPlacer(cache),
            &self.template,
            self.template_position,
            &self.place_settings,
            random,
            seed,
            |_, _, _, _| {},
        );
    }

    fn translate(&mut self, x: i32, y: i32, z: i32) {
        self.move_piece(x, y, z);
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }
}
