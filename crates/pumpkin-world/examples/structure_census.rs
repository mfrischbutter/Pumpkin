//! Counts the structure starts chunk generation would create in a square of chunks,
//! without generating the chunks.
//!
//! Every chunk runs the same per-chunk decision `ProtoChunk::set_structure_starts` does
//! (`chunk_structure_starts`): placement, biomes and the noise height sampler only.
//!
//! ```text
//! cargo run --release -p pumpkin-world --example structure_census -- \
//!     --seed <i64> --dimension <overworld|the_nether|the_end> --radius <chunks> \
//!     [--center <cx> <cz>] [--secret <i64> | --random-secret] [--list]
//! cargo run --release -p pumpkin-world --example structure_census -- //!     --seed <i64> --dimension <overworld|the_nether|the_end> --pieces <cx> <cz> //!     [--secret <i64> | --random-secret]
//! ```
//!
//! Prints one JSON object on stdout: the starts per structure, and with `--list` the
//! start chunks of each. With `--pieces` it prints instead the starts of that one chunk,
//! each with its pieces in generation order (type, bounding box, facing, depth).

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool: the census goes to stdout, usage and timing to stderr"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Instant;

use pumpkin_data::dimension::Dimension;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::generation::generator::VanillaGenerator;
use pumpkin_world::generation::secret::GenerationSecret;
use pumpkin_world::generation::structure::references::chunk_structure_starts;
use pumpkin_world::generation::structure::structures::StructurePosition;
use rayon::prelude::*;

const USAGE: &str = "usage: structure_census --seed <i64> --dimension <overworld|the_nether|the_end> --radius <chunks> [--center <cx> <cz>] [--secret <i64> | --random-secret] [--list]
       structure_census --seed <i64> --dimension <overworld|the_nether|the_end> --pieces <cx> <cz> [--secret <i64> | --random-secret]";

struct Args {
    seed: i64,
    dimension: (&'static str, Dimension),
    radius: i32,
    center: (i32, i32),
    secret: GenerationSecret,
    list: bool,
    /// `--pieces`: the one chunk whose starts are printed with their pieces.
    pieces: Option<(i32, i32)>,
}

fn number<T: FromStr>(args: &mut impl Iterator<Item = String>) -> Result<T, String> {
    let value = args.next().ok_or("missing value")?;
    value.parse().map_err(|_| format!("not a number: {value}"))
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let (mut seed, mut dimension, mut radius) = (None, None, None);
    let mut center = (0, 0);
    let (mut secret, mut random_secret, mut list) = (None, false, false);
    let mut pieces = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => seed = Some(number(&mut args)?),
            "--dimension" => {
                dimension = Some(match args.next().as_deref() {
                    Some("overworld") => ("overworld", Dimension::OVERWORLD),
                    Some("the_nether") => ("the_nether", Dimension::THE_NETHER),
                    Some("the_end") => ("the_end", Dimension::THE_END),
                    other => return Err(format!("unknown dimension: {other:?}")),
                });
            }
            "--radius" => radius = Some(number(&mut args)?),
            "--center" => center = (number(&mut args)?, number(&mut args)?),
            "--secret" => secret = Some(number(&mut args)?),
            "--random-secret" => random_secret = true,
            "--list" => list = true,
            "--pieces" => pieces = Some((number(&mut args)?, number(&mut args)?)),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let radius: i32 = match (radius, pieces) {
        (Some(_), Some(_)) => return Err("--radius and --pieces exclude each other".into()),
        (None, Some(_)) => 0,
        (radius, None) => radius.ok_or("--radius is required")?,
    };
    if radius < 0 {
        return Err("--radius must not be negative".into());
    }
    let secret = match (secret, random_secret) {
        (Some(_), true) => return Err("--secret and --random-secret exclude each other".into()),
        (Some(stored), false) => GenerationSecret::from_stored(Some(stored)),
        (None, true) => GenerationSecret::random(),
        (None, false) => GenerationSecret::NONE,
    };
    Ok(Args {
        seed: seed.ok_or("--seed is required")?,
        dimension: dimension.unwrap_or(("overworld", Dimension::OVERWORLD)),
        radius,
        center,
        secret,
        list,
        pieces,
    })
}

/// The start chunks per structure (`minecraft:<name>`), sorted.
fn census(generator: &VanillaGenerator, args: &Args) -> BTreeMap<String, Vec<(i32, i32)>> {
    let (cx, cz) = args.center;
    let r = args.radius;
    let rows: Vec<Vec<(&'static str, i32, i32)>> = (cz - r..=cz + r)
        .into_par_iter()
        .map(|z| {
            (cx - r..=cx + r)
                .flat_map(|x| {
                    chunk_structure_starts(generator, x, z)
                        .into_iter()
                        .map(move |(key, _)| (key.to_name(), x, z))
                })
                .collect()
        })
        .collect();
    let mut starts: BTreeMap<String, Vec<(i32, i32)>> = BTreeMap::new();
    for (name, x, z) in rows.into_iter().flatten() {
        starts
            .entry(format!("minecraft:{name}"))
            .or_default()
            .push((x, z));
    }
    for chunks in starts.values_mut() {
        chunks.sort_unstable();
    }
    starts
}

fn to_json(args: &Args, starts: &BTreeMap<String, Vec<(i32, i32)>>) -> String {
    let side = 2 * i64::from(args.radius) + 1;
    let secret = secret_json(args);
    let mut json = format!(
        "{{\"seed\": {}, \"dimension\": \"{}\", \"center\": [{}, {}], \"radius\": {}, \"chunks\": {}, \"secret\": {secret}, \"starts\": {{",
        args.seed,
        args.dimension.0,
        args.center.0,
        args.center.1,
        args.radius,
        side * side,
    );
    let counts: Vec<String> = starts
        .iter()
        .map(|(name, chunks)| format!("\"{name}\": {}", chunks.len()))
        .collect();
    json.push_str(&counts.join(", "));
    json.push('}');
    if args.list {
        let lists: Vec<String> = starts
            .iter()
            .map(|(name, chunks)| {
                let mut list = format!("\"{name}\": [");
                for (i, (x, z)) in chunks.iter().enumerate() {
                    let separator = if i == 0 { "" } else { ", " };
                    let _ = write!(list, "{separator}[{x}, {z}]");
                }
                list.push(']');
                list
            })
            .collect();
        let _ = write!(json, ", \"start_chunks\": {{{}}}", lists.join(", "));
    }
    json.push('}');
    json
}

fn secret_json(args: &Args) -> String {
    args.secret
        .stored()
        .map_or_else(|| "null".to_string(), |secret| secret.to_string())
}

/// One start of `--pieces`: its structure, start position and pieces in collector order.
fn start_json(name: &str, start: &StructurePosition) -> String {
    let pos = start.start_pos.0;
    let mut json = format!(
        "{{\"structure\": \"minecraft:{name}\", \"start_pos\": [{}, {}, {}], \"pieces\": [",
        pos.x, pos.y, pos.z
    );
    let collector = start
        .collector
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (i, piece) in collector.pieces.iter().enumerate() {
        let piece = piece.get_structure_piece();
        let (min, max) = (piece.bounding_box.min, piece.bounding_box.max);
        let facing = piece
            .facing
            .map_or_else(|| "null".to_string(), |facing| format!("\"{facing:?}\""));
        let separator = if i == 0 { "" } else { ", " };
        let _ = write!(
            json,
            "{separator}{{\"type\": \"{:?}\", \"bb\": [{}, {}, {}, {}, {}, {}], \"facing\": {facing}, \"depth\": {}}}",
            piece.r#type, min.x, min.y, min.z, max.x, max.y, max.z, piece.chain_length
        );
    }
    json.push_str("]}");
    json
}

/// `--pieces`: the starts of one chunk with their pieces.
fn pieces_json(generator: &VanillaGenerator, args: &Args, (cx, cz): (i32, i32)) -> String {
    let starts: Vec<String> = chunk_structure_starts(generator, cx, cz)
        .iter()
        .map(|(key, start)| start_json(key.to_name(), start))
        .collect();
    format!(
        "{{\"seed\": {}, \"dimension\": \"{}\", \"chunk\": [{cx}, {cz}], \"secret\": {}, \"starts\": [{}]}}",
        args.seed,
        args.dimension.0,
        secret_json(args),
        starts.join(", ")
    )
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let started = Instant::now();
    // Built the way a level builds a default world's dimension generator.
    let generator = VanillaGenerator::new_with_all_settings(
        Seed(args.seed as u64),
        args.dimension.1.clone(),
        None,
        None,
        None,
        args.secret,
    );
    generator
        .global_structure_cache
        .ensure_strongholds_generated();
    if let Some(chunk) = args.pieces {
        println!("{}", pieces_json(&generator, &args, chunk));
        return ExitCode::SUCCESS;
    }
    let starts = census(&generator, &args);
    eprintln!(
        "structure_census: {} chunks in {:.1} s",
        (2 * i64::from(args.radius) + 1).pow(2),
        started.elapsed().as_secs_f64()
    );
    println!("{}", to_json(&args, &starts));
    ExitCode::SUCCESS
}
