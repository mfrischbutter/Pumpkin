use std::{path::PathBuf, sync::Arc};

use pumpkin_config::world::LevelConfig;
use pumpkin_data::dimension::Dimension;

use crate::{generation::secret::GenerationSecret, level::Level};

#[must_use]
pub fn into_level(
    dimension: Dimension,
    level_config: &LevelConfig,
    base_directory: PathBuf,
    seed: i64,
    generation_secret: GenerationSecret,
) -> Arc<Level> {
    Level::from_root_folder(
        level_config,
        base_directory,
        seed,
        generation_secret,
        dimension,
    )
}
