use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use pumpkin_data::poi_type::PoiType;
use pumpkin_data::tag::PointOfInterestType::MINECRAFT_VILLAGE;
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

/// POI type identifier for nether portals
pub const POI_TYPE_NETHER_PORTAL: &str = "minecraft:nether_portal";

/// Vanilla `PoiManager.MAX_VILLAGE_DISTANCE`: a section more steps than this from a village
/// section is not close to one.
pub const MAX_VILLAGE_DISTANCE: i32 = 6;

/// The section `(chunk x, section y, chunk z)` holding a position.
#[must_use]
pub const fn section_of(pos: &BlockPos) -> (i32, i32, i32) {
    (pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4)
}

/// The squared distance of a record from a position.
fn distance_sq(entry: &PoiEntry, center: BlockPos) -> i64 {
    let dx = i64::from(entry.x - center.0.x);
    let dy = i64::from(entry.y - center.0.y);
    let dz = i64::from(entry.z - center.0.z);
    dx * dx + dy * dy + dz * dz
}

/// MCA format constants
const SECTOR_SIZE: usize = 4096;
const REGION_SIZE: usize = 32;
const CHUNK_COUNT: usize = REGION_SIZE * REGION_SIZE;
const HEADER_SIZE: usize = SECTOR_SIZE * 2; // Location table + timestamp table

/// Compression type for MCA format
const COMPRESSION_ZLIB: u8 = 2;

// Data version for 1.21
const DATA_VERSION: i32 = 3955;

/// A single Point of Interest entry (serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoiEntry {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    #[serde(rename = "type")]
    pub poi_type: String,
    pub free_tickets: i32,
}

impl PoiEntry {
    #[must_use]
    pub fn new_portal(pos: BlockPos) -> Self {
        Self {
            x: pos.0.x,
            y: pos.0.y,
            z: pos.0.z,
            poi_type: POI_TYPE_NETHER_PORTAL.to_string(),
            free_tickets: 0,
        }
    }

    #[must_use]
    pub const fn pos(&self) -> BlockPos {
        BlockPos(Vector3::new(self.x, self.y, self.z))
    }

    /// A fresh record of `kind` with every ticket free (vanilla `PoiRecord`).
    #[must_use]
    pub fn new(pos: BlockPos, kind: &'static PoiType) -> Self {
        Self {
            x: pos.0.x,
            y: pos.0.y,
            z: pos.0.z,
            poi_type: format!("minecraft:{}", kind.resource_name),
            free_tickets: max_tickets(kind),
        }
    }

    /// The registered type; `None` for a type this version does not know.
    #[must_use]
    pub fn kind(&self) -> Option<&'static PoiType> {
        PoiType::from_name(&self.poi_type)
    }

    /// Vanilla `PoiRecord.hasSpace`.
    #[must_use]
    pub const fn has_space(&self) -> bool {
        self.free_tickets > 0
    }

    /// Vanilla `PoiRecord.isOccupied`: at least one ticket is taken.
    #[must_use]
    pub fn is_occupied(&self) -> bool {
        self.kind()
            .is_some_and(|kind| self.free_tickets != max_tickets(kind))
    }
}

fn max_tickets(kind: &PoiType) -> i32 {
    i32::try_from(kind.max_tickets).unwrap_or(i32::MAX)
}

/// Which records a lookup accepts (vanilla `PoiManager.Occupancy`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Occupancy {
    HasSpace,
    IsOccupied,
    Any,
}

impl Occupancy {
    fn accepts(self, entry: &PoiEntry) -> bool {
        match self {
            Self::HasSpace => entry.has_space(),
            Self::IsOccupied => entry.is_occupied(),
            Self::Any => true,
        }
    }
}

/// POI section data (serializable) - vanilla format
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PoiSectionData {
    #[serde(default)]
    pub valid: i8,
    #[serde(default)]
    pub records: Vec<PoiEntry>,
}

/// POI chunk data (serializable) - vanilla format
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PoiChunkData {
    pub data_version: i32,
    /// Sections keyed by Y section coordinate (e.g., "-1", "0", "1", "4")
    pub sections: FxHashMap<String, PoiSectionData>,
}

/// POI data for a single region (32x32 chunks) using MCA format
#[derive(Debug, Default)]
pub struct PoiRegion {
    /// Entries indexed by position
    entries: FxHashMap<(i32, i32, i32), PoiEntry>,
    /// Sections whose records match their blocks (vanilla `PoiSection.isValid`), as
    /// `(chunk x, section y, chunk z)`; a section without is scanned when its chunk loads.
    sections: FxHashSet<(i32, i32, i32)>,
    /// Track which chunks are dirty
    dirty_chunks: rustc_hash::FxHashSet<(i32, i32)>,
    dirty: bool,
}

impl PoiRegion {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    const fn pos_key(pos: &BlockPos) -> (i32, i32, i32) {
        (pos.0.x, pos.0.y, pos.0.z)
    }

    /// Get chunk index in MCA file (0-1023)
    const fn chunk_index(chunk_x: i32, chunk_z: i32) -> usize {
        let local_x = chunk_x & 31;
        let local_z = chunk_z & 31;
        ((local_z << 5) | local_x) as usize
    }

    /// Returns section key as just the Y section coordinate (like vanilla)
    fn section_key(pos: &BlockPos) -> String {
        let section_y = pos.0.y >> 4;
        section_y.to_string()
    }

    pub fn add(&mut self, entry: PoiEntry) {
        let chunk_x = entry.x >> 4;
        let chunk_z = entry.z >> 4;
        self.dirty_chunks.insert((chunk_x, chunk_z));
        self.sections.insert(section_of(&entry.pos()));
        let key = (entry.x, entry.y, entry.z);
        self.entries.insert(key, entry);
        self.dirty = true;
    }

    fn has_section(&self, section: (i32, i32, i32)) -> bool {
        self.sections.contains(&section)
    }

    /// The section's records now match its blocks.
    fn mark_section(&mut self, section: (i32, i32, i32)) {
        self.sections.insert(section);
        self.dirty_chunks.insert((section.0, section.2));
        self.dirty = true;
    }

    /// A record at `pos` changed.
    fn touch(&mut self, pos: &BlockPos) {
        self.dirty_chunks.insert((pos.0.x >> 4, pos.0.z >> 4));
        self.dirty = true;
    }

    pub fn remove(&mut self, pos: &BlockPos) -> bool {
        let key = Self::pos_key(pos);
        if self.entries.remove(&key).is_some() {
            let chunk_x = pos.0.x >> 4;
            let chunk_z = pos.0.z >> 4;
            self.dirty_chunks.insert((chunk_x, chunk_z));
            self.dirty = true;
            return true;
        }
        false
    }

    #[must_use]
    pub fn get_all(&self) -> Vec<&PoiEntry> {
        self.entries.values().collect()
    }

    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.dirty_chunks.clear();
    }

    /// Group entries by chunk, then create chunk NBT data
    fn get_chunk_data(&self, chunk_x: i32, chunk_z: i32) -> Option<PoiChunkData> {
        let mut sections: FxHashMap<String, PoiSectionData> = FxHashMap::default();

        // A scanned section without points is saved as valid too, so it is not scanned again.
        for (_, section_y, _) in self
            .sections
            .iter()
            .filter(|(x, _, z)| *x == chunk_x && *z == chunk_z)
        {
            sections.insert(
                section_y.to_string(),
                PoiSectionData {
                    valid: 1,
                    records: Vec::new(),
                },
            );
        }

        for entry in self.entries.values() {
            let entry_chunk_x = entry.x >> 4;
            let entry_chunk_z = entry.z >> 4;

            if entry_chunk_x != chunk_x || entry_chunk_z != chunk_z {
                continue;
            }

            let section_key = Self::section_key(&entry.pos());
            let valid = i8::from(self.sections.contains(&section_of(&entry.pos())));
            let section = sections
                .entry(section_key)
                .or_insert_with(|| PoiSectionData {
                    valid,
                    records: Vec::new(),
                });
            section.records.push(entry.clone());
        }

        if sections.is_empty() {
            None
        } else {
            Some(PoiChunkData {
                data_version: DATA_VERSION,
                sections,
            })
        }
    }

    /// Compress chunk data to bytes
    fn compress_chunk_data(chunk_data: &PoiChunkData) -> std::io::Result<Vec<u8>> {
        let mut root = pumpkin_nbt::compound::NbtCompound::new();
        root.put_int("DataVersion", chunk_data.data_version);

        let mut sections_comp = pumpkin_nbt::compound::NbtCompound::new();
        for (sec_key, sec_data) in &chunk_data.sections {
            let mut sec_comp = pumpkin_nbt::compound::NbtCompound::new();
            sec_comp.put_byte("Valid", sec_data.valid);
            let mut rec_list = Vec::new();
            for rec in &sec_data.records {
                let mut rec_comp = pumpkin_nbt::compound::NbtCompound::new();
                rec_comp.put_int("x", rec.x);
                rec_comp.put_int("y", rec.y);
                rec_comp.put_int("z", rec.z);
                rec_comp.put_string("type", rec.poi_type.clone());
                rec_comp.put_int("free_tickets", rec.free_tickets);
                rec_list.push(pumpkin_nbt::tag::NbtTag::Compound(rec_comp));
            }
            sec_comp.put_list("Records", rec_list);
            sections_comp.put_compound(sec_key, sec_comp);
        }
        root.put_compound("Sections", sections_comp);

        let uncompressed = pumpkin_nbt::Nbt::from(root).write();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&uncompressed)?;
        encoder.finish()
    }

    /// Decompress chunk data from bytes
    fn decompress_chunk_data(compressed: &[u8]) -> std::io::Result<PoiChunkData> {
        let mut decoder = ZlibDecoder::new(compressed);
        let mut uncompressed = Vec::new();
        decoder.read_to_end(&mut uncompressed)?;

        let mut cursor = Cursor::new(uncompressed);
        let mut reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(
            pumpkin_nbt::deserializer::NbtStreamReader(&mut cursor),
        );
        let nbt = pumpkin_nbt::Nbt::read(&mut reader)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

        let data_version = nbt.get_int("DataVersion").unwrap_or(DATA_VERSION);
        let mut sections = FxHashMap::default();

        if let Some(sec_tag) = nbt.get_compound("Sections") {
            for (sec_key, tag) in &sec_tag.child_tags {
                if let pumpkin_nbt::tag::NbtTag::Compound(sec_comp) = tag {
                    let valid = sec_comp.get_byte("Valid").unwrap_or(1);
                    let mut records = Vec::new();
                    if let Some(pumpkin_nbt::tag::NbtTag::List(rec_list)) = sec_comp.get("Records")
                    {
                        for rec_t in rec_list {
                            if let pumpkin_nbt::tag::NbtTag::Compound(rc) = rec_t {
                                records.push(PoiEntry {
                                    x: rc.get_int("x").unwrap_or(0),
                                    y: rc.get_int("y").unwrap_or(0),
                                    z: rc.get_int("z").unwrap_or(0),
                                    poi_type: rc
                                        .get_string("type")
                                        .unwrap_or(POI_TYPE_NETHER_PORTAL)
                                        .to_string(),
                                    free_tickets: rc.get_int("free_tickets").unwrap_or(0),
                                });
                            }
                        }
                    }
                    sections.insert(sec_key.to_string(), PoiSectionData { valid, records });
                }
            }
        }

        Ok(PoiChunkData {
            data_version,
            sections,
        })
    }

    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }

        if self.entries.is_empty() && self.sections.is_empty() {
            // Don't save empty regions, delete the file if it exists
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            self.dirty = false;
            self.dirty_chunks.clear();
            return Ok(());
        }

        // Build all chunk data
        let mut chunk_data_map: FxHashMap<usize, Vec<u8>> = FxHashMap::default();

        // Collect all unique chunks that have entries
        let mut chunks_with_data: rustc_hash::FxHashSet<(i32, i32)> =
            rustc_hash::FxHashSet::default();
        for entry in self.entries.values() {
            chunks_with_data.insert((entry.x >> 4, entry.z >> 4));
        }
        for (chunk_x, _, chunk_z) in &self.sections {
            chunks_with_data.insert((*chunk_x, *chunk_z));
        }

        for (chunk_x, chunk_z) in &chunks_with_data {
            if let Some(chunk_data) = self.get_chunk_data(*chunk_x, *chunk_z) {
                let compressed = Self::compress_chunk_data(&chunk_data)?;
                let index = Self::chunk_index(*chunk_x, *chunk_z);
                chunk_data_map.insert(index, compressed);
            }
        }

        // Build MCA file
        let mut location_table = [0u32; CHUNK_COUNT];
        let mut timestamp_table = [0u32; CHUNK_COUNT];
        let mut sector_data: Vec<Vec<u8>> = Vec::new();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as u32);

        // Start after header (2 sectors)
        let mut current_sector: u32 = 2;

        for index in 0..CHUNK_COUNT {
            if let Some(compressed) = chunk_data_map.get(&index) {
                // Calculate sector count needed
                let data_len = compressed.len() + 5; // 4 bytes length + 1 byte compression + data
                let sector_count = data_len.div_ceil(SECTOR_SIZE) as u32;

                // Build padded sector data
                let mut padded = Vec::with_capacity(sector_count as usize * SECTOR_SIZE);
                let length = (compressed.len() + 1) as u32; // +1 for compression byte
                padded.extend_from_slice(&length.to_be_bytes());
                padded.push(COMPRESSION_ZLIB);
                padded.extend_from_slice(compressed);
                // Pad to sector boundary
                padded.resize(sector_count as usize * SECTOR_SIZE, 0);

                location_table[index] = (current_sector << 8) | sector_count;
                timestamp_table[index] = timestamp;
                sector_data.push(padded);

                current_sector += sector_count;
            }
        }

        // Write file
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::File::create(path)?;

        // Write location table
        for loc in &location_table {
            file.write_all(&loc.to_be_bytes())?;
        }

        // Write timestamp table
        for ts in &timestamp_table {
            file.write_all(&ts.to_be_bytes())?;
        }

        // Write chunk data
        for data in &sector_data {
            file.write_all(data)?;
        }

        self.dirty = false;
        self.dirty_chunks.clear();
        Ok(())
    }

    pub fn load(path: &Path, region_x: i32, region_z: i32) -> std::io::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let file_data = std::fs::read(path)?;
        if file_data.len() < HEADER_SIZE {
            return Ok(Self::new());
        }

        let mut region = Self::new();

        // Parse location table
        for index in 0..CHUNK_COUNT {
            let offset = index * 4;
            let location = u32::from_be_bytes([
                file_data[offset],
                file_data[offset + 1],
                file_data[offset + 2],
                file_data[offset + 3],
            ]);

            let sector_offset = (location >> 8) as usize;
            let sector_count = (location & 0xFF) as usize;

            if sector_offset == 0 || sector_count == 0 {
                continue;
            }

            let byte_offset = sector_offset * SECTOR_SIZE;
            let byte_end = byte_offset + sector_count * SECTOR_SIZE;

            if byte_end > file_data.len() {
                continue;
            }

            // Read chunk data
            let chunk_bytes = &file_data[byte_offset..byte_end];
            if chunk_bytes.len() < 5 {
                continue;
            }

            let length = u32::from_be_bytes([
                chunk_bytes[0],
                chunk_bytes[1],
                chunk_bytes[2],
                chunk_bytes[3],
            ]) as usize;
            let compression = chunk_bytes[4];

            if compression != COMPRESSION_ZLIB || length < 1 || length > chunk_bytes.len() - 4 {
                continue;
            }

            let compressed = &chunk_bytes[5..5 + length - 1];

            match Self::decompress_chunk_data(compressed) {
                Ok(chunk_data) => {
                    let chunk_x = (region_x << 5) | (index & 31) as i32;
                    let chunk_z = (region_z << 5) | (index >> 5) as i32;
                    for (section_key, section) in chunk_data.sections {
                        if section.valid != 0
                            && let Ok(section_y) = section_key.parse::<i32>()
                        {
                            region.sections.insert((chunk_x, section_y, chunk_z));
                        }
                        for entry in section.records {
                            let key = (entry.x, entry.y, entry.z);
                            region.entries.insert(key, entry);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to parse POI chunk at index {index}: {e}");
                }
            }
        }

        region.dirty = false;
        Ok(region)
    }
}

/// Region-based POI storage using MCA format
pub struct PoiStorage {
    /// Path to the poi folder
    folder: PathBuf,
    /// Loaded regions, keyed by (`region_x`, `region_z`)
    regions: FxHashMap<(i32, i32), PoiRegion>,
    /// Vanilla's village distance tracker: the sections within `MAX_VILLAGE_DISTANCE` of a
    /// village section with their distance, recomputed after a change when next asked.
    village_levels: Option<FxHashMap<(i32, i32, i32), i32>>,
}

impl PoiStorage {
    #[must_use]
    pub fn new(poi_folder: PathBuf) -> Self {
        Self {
            folder: poi_folder,
            regions: FxHashMap::default(),
            village_levels: None,
        }
    }

    const fn region_coords(pos: &BlockPos) -> (i32, i32) {
        let chunk_x = pos.0.x >> 4;
        let chunk_z = pos.0.z >> 4;
        (chunk_x >> 5, chunk_z >> 5)
    }

    fn region_path(&self, rx: i32, rz: i32) -> PathBuf {
        self.folder.join(format!("r.{rx}.{rz}.mca"))
    }

    fn get_or_load_region(&mut self, rx: i32, rz: i32) -> &mut PoiRegion {
        let path = self.region_path(rx, rz);
        self.regions.entry((rx, rz)).or_insert_with(|| {
            PoiRegion::load(&path, rx, rz).unwrap_or_else(|e| {
                if path.exists() {
                    warn!("Failed to load POI region {}: {}", path.display(), e);
                }
                PoiRegion::new()
            })
        })
    }

    pub fn add(&mut self, pos: BlockPos, poi_type: &str) {
        self.add_with_free_tickets(pos, poi_type, 0);
    }

    pub fn add_with_free_tickets(&mut self, pos: BlockPos, poi_type: &str, free_tickets: i32) {
        let (rx, rz) = Self::region_coords(&pos);
        let region = self.get_or_load_region(rx, rz);
        region.add(PoiEntry {
            x: pos.0.x,
            y: pos.0.y,
            z: pos.0.z,
            poi_type: poi_type.to_string(),
            free_tickets,
        });
        self.village_levels = None;
    }

    pub fn add_portal(&mut self, pos: BlockPos) {
        self.add(pos, POI_TYPE_NETHER_PORTAL);
    }

    pub fn remove(&mut self, pos: &BlockPos) -> bool {
        let (rx, rz) = Self::region_coords(pos);
        let removed = self.get_or_load_region(rx, rz).remove(pos);
        self.village_levels = None;
        removed
    }

    /// Vanilla `PoiManager.add` for a registered type: a fresh record with every ticket
    /// free; a record of the same type already at `pos` keeps its tickets.
    pub fn add_type(&mut self, pos: BlockPos, kind: &'static PoiType) {
        let (rx, rz) = Self::region_coords(&pos);
        let region = self.get_or_load_region(rx, rz);
        if region
            .entries
            .get(&PoiRegion::pos_key(&pos))
            .and_then(PoiEntry::kind)
            == Some(kind)
        {
            return;
        }
        region.add(PoiEntry::new(pos, kind));
        self.village_levels = None;
    }

    /// Whether a section already holds point of interest data (vanilla
    /// `PoiSection.isValid`); one without is scanned when its chunk loads.
    pub fn section_has_data(&mut self, section: (i32, i32, i32)) -> bool {
        self.get_or_load_region(section.0 >> 5, section.2 >> 5)
            .has_section(section)
    }

    /// Vanilla `PoiSection.refresh` with the points the section's blocks hold: `found`
    /// becomes the section's records; a record still at its position keeps its tickets.
    pub fn refresh_section(
        &mut self,
        section: (i32, i32, i32),
        found: Vec<(BlockPos, &'static PoiType)>,
    ) {
        let region = self.get_or_load_region(section.0 >> 5, section.2 >> 5);
        let mut old: FxHashMap<_, _> = region
            .entries
            .extract_if(|(x, y, z), _| (x >> 4, y >> 4, z >> 4) == section)
            .collect();
        for (pos, kind) in found {
            let entry = old
                .remove(&PoiRegion::pos_key(&pos))
                .unwrap_or_else(|| PoiEntry::new(pos, kind));
            region.add(entry);
        }
        region.mark_section(section);
        self.village_levels = None;
    }

    /// The regions a square of `radius` blocks around `center` reaches into.
    fn regions_around(center: BlockPos, radius: i32) -> impl Iterator<Item = (i32, i32)> {
        let min_rx = ((center.0.x - radius) >> 4) >> 5;
        let max_rx = ((center.0.x + radius) >> 4) >> 5;
        let min_rz = ((center.0.z - radius) >> 4) >> 5;
        let max_rz = ((center.0.z + radius) >> 4) >> 5;
        (min_rx..=max_rx).flat_map(move |rx| (min_rz..=max_rz).map(move |rz| (rx, rz)))
    }

    /// Vanilla `PoiManager.getInRange`: the points within `radius` blocks of `center`
    /// whose type passes `kind` and whose tickets pass `occupancy`.
    pub fn in_range(
        &mut self,
        center: BlockPos,
        radius: i32,
        kind: impl Fn(&'static PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<(BlockPos, &'static PoiType)> {
        let radius_sq = i64::from(radius).pow(2);
        let mut found = Vec::new();
        for (rx, rz) in Self::regions_around(center, radius) {
            let region = self.get_or_load_region(rx, rz);
            for entry in region.entries.values() {
                if (entry.x - center.0.x).abs() > radius || (entry.z - center.0.z).abs() > radius {
                    continue;
                }
                let Some(entry_kind) = entry.kind() else {
                    continue;
                };
                if !kind(entry_kind)
                    || !occupancy.accepts(entry)
                    || distance_sq(entry, center) > radius_sq
                {
                    continue;
                }
                found.push((entry.pos(), entry_kind));
            }
        }
        found
    }

    /// Vanilla `PoiManager.getCountInRange`.
    pub fn count_in_range(
        &mut self,
        center: BlockPos,
        radius: i32,
        kind: impl Fn(&'static PoiType) -> bool,
        occupancy: Occupancy,
    ) -> usize {
        self.in_range(center, radius, kind, occupancy).len()
    }

    /// Vanilla `PoiManager.findClosest` with a position filter: the nearest point of
    /// [`Self::in_range`] whose position passes `filter`.
    pub fn find_closest(
        &mut self,
        center: BlockPos,
        radius: i32,
        kind: impl Fn(&'static PoiType) -> bool,
        filter: impl Fn(&BlockPos) -> bool,
        occupancy: Occupancy,
    ) -> Option<BlockPos> {
        self.in_range(center, radius, kind, occupancy)
            .into_iter()
            .map(|(pos, _)| pos)
            .filter(|pos| filter(pos))
            .min_by_key(|pos| {
                let delta = pos.0 - center.0;
                i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
            })
    }

    /// Vanilla `PoiManager.find`: any point of [`Self::in_range`] whose position passes
    /// `filter`.
    pub fn find_any(
        &mut self,
        center: BlockPos,
        radius: i32,
        kind: impl Fn(&'static PoiType) -> bool,
        filter: impl Fn(&BlockPos) -> bool,
        occupancy: Occupancy,
    ) -> Option<BlockPos> {
        self.in_range(center, radius, kind, occupancy)
            .into_iter()
            .map(|(pos, _)| pos)
            .find(|pos| filter(pos))
    }

    /// Vanilla `PoiRecord.acquireTicket`: takes one of the point's tickets.
    pub fn acquire_ticket(&mut self, pos: BlockPos) -> bool {
        let (rx, rz) = Self::region_coords(&pos);
        let region = self.get_or_load_region(rx, rz);
        let Some(entry) = region.entries.get_mut(&PoiRegion::pos_key(&pos)) else {
            return false;
        };
        if entry.free_tickets <= 0 {
            return false;
        }
        entry.free_tickets -= 1;
        region.touch(&pos);
        self.village_levels = None;
        true
    }

    /// Vanilla `PoiManager.release`: gives one ticket back; false when none was taken.
    pub fn release_ticket(&mut self, pos: BlockPos) -> bool {
        let (rx, rz) = Self::region_coords(&pos);
        let region = self.get_or_load_region(rx, rz);
        let Some(entry) = region.entries.get_mut(&PoiRegion::pos_key(&pos)) else {
            return false;
        };
        if entry
            .kind()
            .is_none_or(|kind| entry.free_tickets >= max_tickets(kind))
        {
            return false;
        }
        entry.free_tickets += 1;
        region.touch(&pos);
        self.village_levels = None;
        true
    }

    /// The type of the point at `pos`, if there is one (vanilla `PoiManager.getType`).
    pub fn get_type(&mut self, pos: BlockPos) -> Option<&'static PoiType> {
        let (rx, rz) = Self::region_coords(&pos);
        self.get_or_load_region(rx, rz)
            .entries
            .get(&PoiRegion::pos_key(&pos))
            .and_then(PoiEntry::kind)
    }

    /// Vanilla `PoiManager.sectionsToVillage`: how many sections (26-neighbour steps) the
    /// nearest village section is away, 0 inside one and `MAX_VILLAGE_DISTANCE + 1` when none
    /// is within reach. A village section holds an occupied point of the `village` tag.
    pub fn sections_to_village(&mut self, section: (i32, i32, i32)) -> i32 {
        if self.village_levels.is_none() {
            let levels = self.compute_village_levels();
            self.village_levels = Some(levels);
        }
        self.village_levels
            .as_ref()
            .and_then(|levels| levels.get(&section))
            .copied()
            .unwrap_or(MAX_VILLAGE_DISTANCE + 1)
    }

    fn compute_village_levels(&self) -> FxHashMap<(i32, i32, i32), i32> {
        let mut levels = FxHashMap::default();
        let mut frontier: Vec<(i32, i32, i32)> = self
            .regions
            .values()
            .flat_map(|region| region.entries.values())
            .filter(|entry| {
                entry.is_occupied()
                    && entry
                        .kind()
                        .is_some_and(|kind| kind.has_tag(&MINECRAFT_VILLAGE))
            })
            .map(|entry| section_of(&entry.pos()))
            .collect();
        for section in &frontier {
            levels.insert(*section, 0);
        }
        for level in 1..=MAX_VILLAGE_DISTANCE {
            let mut next = Vec::new();
            for (x, y, z) in frontier {
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            let neighbour = (x + dx, y + dy, z + dz);
                            if let Entry::Vacant(entry) = levels.entry(neighbour) {
                                entry.insert(level);
                                next.push(neighbour);
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        levels
    }

    /// Get all POI positions within a square radius (for portal search)
    #[expect(clippy::similar_names)]
    pub fn get_in_square(
        &mut self,
        center: BlockPos,
        radius: i32,
        poi_type: Option<&str>,
    ) -> Vec<BlockPos> {
        let min_x = center.0.x - radius;
        let max_x = center.0.x + radius;
        let min_z = center.0.z - radius;
        let max_z = center.0.z + radius;

        // Calculate which regions we need to check
        let min_rx = (min_x >> 4) >> 5;
        let max_rx = (max_x >> 4) >> 5;
        let min_rz = (min_z >> 4) >> 5;
        let max_rz = (max_z >> 4) >> 5;

        let mut results = Vec::new();

        for rx in min_rx..=max_rx {
            for rz in min_rz..=max_rz {
                let region = self.get_or_load_region(rx, rz);
                for entry in region.get_all() {
                    if let Some(filter_type) = poi_type
                        && entry.poi_type != filter_type
                    {
                        continue;
                    }

                    let dx = (entry.x - center.0.x).abs();
                    let dz = (entry.z - center.0.z).abs();
                    if dx <= radius && dz <= radius {
                        results.push(entry.pos());
                    }
                }
            }
        }

        results
    }

    /// Finds the closest POI whose type matches `matches`, considering
    /// entries within `radius` blocks of `center` on the x/z axes (like
    /// vanilla's `PoiManager.findClosestWithType`: a chebyshev square gather
    /// followed by picking the smallest 3D squared distance).
    ///
    /// Returns the entry's position together with its type.
    pub fn find_closest_matching(
        &mut self,
        center: BlockPos,
        radius: i32,
        matches: impl Fn(&str) -> bool,
    ) -> Option<(BlockPos, String)> {
        let min_rx = ((center.0.x - radius) >> 4) >> 5;
        let max_rx = ((center.0.x + radius) >> 4) >> 5;
        let min_rz = ((center.0.z - radius) >> 4) >> 5;
        let max_rz = ((center.0.z + radius) >> 4) >> 5;

        let mut best: Option<(BlockPos, String, i64)> = None;

        for rx in min_rx..=max_rx {
            for rz in min_rz..=max_rz {
                let region = self.get_or_load_region(rx, rz);
                for entry in region.get_all() {
                    if (entry.x - center.0.x).abs() > radius
                        || (entry.z - center.0.z).abs() > radius
                        || !matches(&entry.poi_type)
                    {
                        continue;
                    }

                    let dx = i64::from(entry.x - center.0.x);
                    let dy = i64::from(entry.y - center.0.y);
                    let dz = i64::from(entry.z - center.0.z);
                    let distance_sq = dx * dx + dy * dy + dz * dz;

                    if best.as_ref().is_none_or(|(_, _, d)| distance_sq < *d) {
                        best = Some((entry.pos(), entry.poi_type.clone(), distance_sq));
                    }
                }
            }
        }

        best.map(|(pos, poi_type, _)| (pos, poi_type))
    }

    pub fn save_all(&mut self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.folder)?;

        let mut saved = 0;
        for ((rx, rz), region) in &mut self.regions {
            if region.is_dirty() {
                let path = self.folder.join(format!("r.{rx}.{rz}.mca"));
                region.save(&path)?;
                saved += 1;
            }
        }

        if saved > 0 {
            info!("Saved {saved} POI region(s)");
        }
        Ok(())
    }

    /// Get count of loaded regions
    #[must_use]
    pub fn loaded_region_count(&self) -> usize {
        self.regions.len()
    }

    /// Get total POI count across all loaded regions
    #[must_use]
    pub fn total_poi_count(&self) -> usize {
        self.regions.values().map(|r| r.get_all().len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poi_entry() {
        let entry = PoiEntry::new_portal(BlockPos(Vector3::new(100, 64, 200)));
        assert_eq!(entry.x, 100);
        assert_eq!(entry.y, 64);
        assert_eq!(entry.z, 200);
        assert_eq!(entry.poi_type, POI_TYPE_NETHER_PORTAL);
    }

    #[test]
    fn poi_region() {
        let mut region = PoiRegion::new();
        region.add(PoiEntry::new_portal(BlockPos(Vector3::new(100, 64, 200))));
        region.add(PoiEntry::new_portal(BlockPos(Vector3::new(101, 64, 200))));

        assert_eq!(region.get_all().len(), 2);
        assert!(region.is_dirty());

        region.remove(&BlockPos(Vector3::new(100, 64, 200)));
        assert_eq!(region.get_all().len(), 1);
    }

    #[test]
    fn poi_find_closest_matching() {
        let mut storage = PoiStorage::new(std::env::temp_dir().join("pumpkin_poi_closest_test"));

        storage.add_portal(BlockPos(Vector3::new(100, 64, 100)));
        storage.add_portal(BlockPos(Vector3::new(120, 64, 100)));
        storage.add(BlockPos(Vector3::new(101, 64, 100)), "minecraft:home");

        let center = BlockPos(Vector3::new(105, 64, 100));
        let (pos, poi_type) = storage
            .find_closest_matching(center, 256, |t| t == POI_TYPE_NETHER_PORTAL)
            .unwrap();
        assert_eq!(pos, BlockPos(Vector3::new(100, 64, 100)));
        assert_eq!(poi_type, POI_TYPE_NETHER_PORTAL);

        // The overall closest one ignores the type filter mismatch above.
        let (pos, poi_type) = storage
            .find_closest_matching(center, 256, |_| true)
            .unwrap();
        assert_eq!(pos, BlockPos(Vector3::new(101, 64, 100)));
        assert_eq!(poi_type, "minecraft:home");

        assert!(
            storage
                .find_closest_matching(center, 256, |t| t == "minecraft:lodestone")
                .is_none()
        );
        // Out of horizontal range.
        assert!(
            storage
                .find_closest_matching(BlockPos(Vector3::new(1000, 64, 100)), 16, |_| true)
                .is_none()
        );
    }

    /// Occupied village points make village sections; the distance counts 26-neighbour
    /// steps up to `MAX_VILLAGE_DISTANCE`.
    #[test]
    fn village_distance_counts_sections_from_occupied_points() {
        let mut storage = PoiStorage::new(std::env::temp_dir().join("pumpkin_poi_village_test"));
        let bed = BlockPos(Vector3::new(8, 64, 8));
        storage.add_type(bed, &PoiType::HOME);
        assert_eq!(
            storage.sections_to_village((0, 4, 0)),
            MAX_VILLAGE_DISTANCE + 1
        );
        assert!(storage.acquire_ticket(bed));
        assert_eq!(storage.sections_to_village((0, 4, 0)), 0);
        assert_eq!(storage.sections_to_village((1, 5, 1)), 1);
        assert_eq!(storage.sections_to_village((6, 4, 0)), MAX_VILLAGE_DISTANCE);
        assert_eq!(
            storage.sections_to_village((7, 4, 0)),
            MAX_VILLAGE_DISTANCE + 1
        );
        assert!(storage.release_ticket(bed));
        assert!(!storage.release_ticket(bed));
        assert_eq!(
            storage.sections_to_village((0, 4, 0)),
            MAX_VILLAGE_DISTANCE + 1
        );
    }

    /// A rescan keeps the tickets of a point that is still there and drops the rest.
    #[test]
    fn refresh_keeps_tickets_of_points_still_there() {
        let mut storage = PoiStorage::new(std::env::temp_dir().join("pumpkin_poi_refresh_test"));
        let kept = BlockPos(Vector3::new(1, 65, 1));
        let gone = BlockPos(Vector3::new(2, 65, 2));
        storage.add_type(kept, &PoiType::HOME);
        storage.add_type(gone, &PoiType::HOME);
        assert!(storage.acquire_ticket(kept));
        assert!(!storage.section_has_data((5, 4, 5)));
        storage.refresh_section((0, 4, 0), vec![(kept, &PoiType::HOME)]);
        assert!(storage.section_has_data((0, 4, 0)));
        assert_eq!(storage.get_type(gone), None);
        assert!(!storage.acquire_ticket(kept));
        assert_eq!(
            storage.count_in_range(kept, 8, |_| true, Occupancy::IsOccupied),
            1
        );
    }

    #[test]
    fn poi_storage_mca() {
        let dir = std::env::temp_dir().join("pumpkin_poi_mca_test");
        let _ = std::fs::remove_dir_all(&dir);

        let mut storage = PoiStorage::new(dir.join("poi"));

        storage.add_portal(BlockPos(Vector3::new(100, 64, 100)));
        storage.add_portal(BlockPos(Vector3::new(110, 64, 100)));
        storage.add_portal(BlockPos(Vector3::new(1000, 64, 1000))); // Different region

        let results = storage.get_in_square(
            BlockPos(Vector3::new(105, 64, 100)),
            16,
            Some(POI_TYPE_NETHER_PORTAL),
        );
        assert_eq!(results.len(), 2);

        storage.save_all().unwrap();

        // Verify .mca file was created
        let mca_path = dir.join("poi").join("r.0.0.mca");
        assert!(mca_path.exists());

        // Reload and verify
        let mut storage2 = PoiStorage::new(dir.join("poi"));
        let results2 = storage2.get_in_square(
            BlockPos(Vector3::new(105, 64, 100)),
            16,
            Some(POI_TYPE_NETHER_PORTAL),
        );
        assert_eq!(results2.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
