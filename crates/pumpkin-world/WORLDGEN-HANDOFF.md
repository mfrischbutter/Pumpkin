# World generation: hand-off for the next run toward 100 % vanilla

For the next worker on this fork (`mfrischbutter/Pumpkin`, branch `cubeattack`) who continues the
vanilla parity of world generation. Written at the end of job F-9 (pull request #1, November 2026). It
supersedes "Where it stands" and "Open work" of CubeAttack's `docs/worldgen-handoff.md` (F-7); that file
and `docs/worldgen-next-steps.md` keep the older evidence. The goal stays the same: for the same seed
the fork generates what the vanilla 26.3 server generates; with the generation secret, the same
terrain and biomes and the same distributions of everything else.

## Read first

- CubeAttack, which holds the tools and the measurement history. The job's repository notes name its
  checkout on the runner machine; that checkout may lag behind its `main`, so clone its `origin` into a
  scratch folder for the current one. In it: `docs/worldgen-compare.md` (the verification and every
  round's results),
  `docs/worldgen-next-steps.md` (evidence per item), `docs/worldgen-handoff.md` (F-7: the decoration
  order findings, the clay pool, the proto chunk tools), `docs/seed-cracking-check.md` and
  `docs/deployment.md` "World seed and generation secret".
- Vanilla's source: Lantern (`docs/lantern.md` in CubeAttack). The decompiled 26.3 server is on the
  runner machine under `%APPDATA%\lantern\versions\26.3\src`.
- This fork's rules: `AGENTS.md`, `crates/pumpkin-world/AGENTS.md`.

## Where it stands

CubeAttack's verification: 4 seeds (1657324430, -4172144997902289642, 8095405736471581734,
7853492499715517705), 217 areas (the Nether and the End around 0 0, 17x17 chunks around one of each
structure kind vanilla's `locate` finds, the 18 abandoned camps, an 81x81 chunk census area per seed),
three vanilla 26.3 runs each. An area passes when, at the positions where vanilla-1 and vanilla-2 agree,
it differs from them by no more than twice what vanilla-3 does plus 64 blocks per block category and per
block name, in no biome cell, in block entities and entities by no more than twice the control plus 2
per kind, and in structure references by no more than the control.

| Build | Areas passing (of 217) | Failing on generated blocks | Failing on fluids, items or mobs only | Differing positions |
|---|---|---|---|---|
| REI-13 second round (21c340c6e) | 66 (30 %) | 108 | 43 | 4,854,601 |
| pin `cubeattack-2026-11-07-2` (0ec654806) | 91 (42 %) | | | |
| f-4-worldgen (5bc40b886) | 99 (46 %) | 97 | 21 | 5,253,074 |
| f-5-worldgen (26184f72d) | 104 (48 %) | 92 | 21 | 5,218,005 |
| F-9, pull request #1 (image of 112d7482c) | 112 (52 %) | 80 | 25 | 4,628,550 |
| vanilla-3, the control | | | | 5,203,150 |

After F-9:

- Exact: biomes (0 differing cells), structure placement (every chunk's structure references are
  vanilla's: 0 missing, 0 extra), the End (8 of 8 areas).
- At the positions where vanilla-1 and vanilla-2 agree, 99.934 % are identical; the 0.0655 % that differ
  are fewer than vanilla-3 differs there (0.0737 %). Summed over all areas every block category is below
  vanilla-3's: tree 2.07 M against 2.31 M, vegetation 1.24 M against 1.33 M, terrain 0.58 M against
  0.62 M, dripstone 0.30 M against 0.46 M, sculk 0.16 M against 0.17 M, ore 0.13 M against 0.15 M, fluid
  0.11 M against 0.12 M.
- Where the fork is furthest from vanilla's noise: all four 81x81 census areas fail. They hold 34.7 % of
  the compared positions and 1.45 M of the fork's 4.63 M differing positions (vanilla-3: 1.24 M of
  5.20 M). Seed 7853492499715517705's census area alone has 1.01 M (vanilla-3: 0.83 M), with terrain
  99,821 against vanilla-3's 32,315, ore 48,989 against 14,700 and sculk 14,173 against 6,821. Run on
  their own, seeds 8095405736471581734 and 7853492499715517705 fail the category totals on terrain
  (233,479 against an allowed 218,172) and sculk (75,346 against 72,132). F-9 lowered all four census
  areas (that one from 1,145,729 on f-5-worldgen).
- Per area 112 of 217 pass, give or take a few between runs (areas near the threshold flip on wandering
  cats or items). 25 of the 105 failing areas fail only on fluids, items or mobs. 24 of them count as
  runtime, not generation, by F-7's attribution (not traced per area); the 25th misses vanilla's 4
  generated cows. Without the 24, 136 of 217 (63 %) are within vanilla's noise.
- Block entities, at the positions where vanilla-1 and vanilla-2 agree (what the verification compares):
  chests 3,691 against vanilla-1's 3,691, spawners 2,110 against 2,114, trial spawners 903 against 903,
  vaults 1,395 against 1,396. Sculk sensors, catalysts and shriekers and creaking hearts differ from
  vanilla-1 about as much as vanilla-3 does (sensors: the fork 325 missing and 1,565 extra, vanilla-3
  428 and 1,479; creaking hearts: 35 and 50 differences). They come from decoration, so like the
  decoration blocks they fail only in single areas: sculk sensors in 7, catalysts in 4.
- Decorated proto chunks (no ticks, 480 sampled from vanilla's runs), with proto_parity's model of the
  order (not the scheduler itself): 253 identical to vanilla block for block in the order the scheduler
  now builds in (f-5-worldgen's x-then-z order: 127); 316 with the chunks two away decorated too (150).
- Seed-cracking check of F-9's build: passed, see "Seed-cracking check" below.

"100 %" block for block is not reachable even between two vanilla runs: vanilla's decoration order
follows thread timing (vanilla-1 and vanilla-2 disagree at 0.167 % of the positions). The reachable goal
is every area within vanilla's own noise. The verification measures force-loaded areas; in play,
chunks load around moving players and no fixed order matches vanilla block for block in either server.
With the generation secret, terrain and biomes stay the seed's; structures, caves and features can only
match vanilla in distribution. The verification checks that distribution for structures and dungeons
(with the secret, the census of their starts, chests and spawners and the starts in the census areas
are within placement noise), not for caves or the other features. F-9's run also lacked the large-area
start census, which needs the census tool (a native build); it passed on f-5-worldgen, whose placement
code F-9 carries unchanged.

## What F-9 changed, and what it taught

- Landed f-5-worldgen's seven fixes (vanilla's `liquid()`, the buried treasure loot seed, brushable
  blocks, double plants at tree edges, lava without drops, the aquifer, the splines).
- The decoration order (`chunk_system/schedule.rs`, `features_order_key`): a chunk's features wait for
  every chunk within two that is to be decorated, not yet decorated, and earlier in a model of vanilla's
  generation: a chunk is decorated once the last chunk of its 3x3 has its terrain, and a chunk's terrain
  comes with the lowest ticket level within `FULL_TERRAIN_RADIUS` (2) of it (vanilla's full-chunk
  `ChunkGenerationTask`s generate the terrain two chunks around them, and the dispatcher runs lower
  levels first); ties go by x, then z. Keys are cached until the levels change.
- Lesson: the proto chunk measurement samples only the ring of decorated chunks around vanilla's
  areas. F-9's first model (each terrain chunk's own ticket level) scored the same there (253 and 316)
  and failed the server verification (86 of 108 areas on two seeds, against f-5-worldgen's 56 and the
  final model's 52): inside a force-loaded area every chunk has the same level, so it put the area's
  edge rows after the inside, while vanilla's forced tasks decorate their 3x3 x first. Always verify an
  order or scheduler change with a server run.
- Lesson: the verification compares block entities only where vanilla-1 and vanilla-2 agree, so its
  "reference" count holds only the block entities both runs share, while another run's extra ones still
  count wherever neither of the two has one. Compare the fork's missing and extra with vanilla-3's
  (`control_block_entities` in the report's JSON), never with the reference count alone: the sculk
  sensors looked like 1,242 too many until vanilla-3 showed 1,059 more as well.
- Mineshaft rails only on floors that are solid render (`MineShaftCorridor.postProcess`).
- A Criterion bench of the scheduler, `crates/pumpkin-world/benches/chunk_schedule.rs` (a player's view
  of 8 and a forced 16x16 area through `Level`'s tickets). Run before and after any scheduler change; the
  machine drifts, so run both back to back. F-9: the player's view unchanged (+0.4 %, p = 0.65), the
  forced area 24 % faster.
- CI: GitHub CI builds with the current Rust stable and `RUSTFLAGS=-Dwarnings`; CubeAttack's check image
  pins 1.99.1. When stable moved to 1.100 (2026-11-12) the PR had to take rustfmt's and clippy's new
  findings across the workspace. The GameTest arena of `tools/pumpkin-tester` now uses the forced ticket
  (`add_forced_chunk_ticket`). With the full-chunk ticket its edge chunks never ticked, since chunks
  tick only with their eight neighbours loaded, and 57 of 200 GameTests failed, on `cubeattack` too.

## How to measure (there is no local Rust toolchain)

Every cargo run happens in Docker through CubeAttack's scripts, with `PUMPKIN_SRC` set to the worktree.

1. **Checks:** `CA=<cubeattack checkout>; PUMPKIN_SRC="$(pwd -W)" bash "$CA/scripts/pumpkin-check.sh"
   <crates>` (fmt, clippy, nextest, machete, typos), `--boot`, `--image` (a local image
   `cubeattack/pumpkin-local:<sha>` with its boot test). `--image` replaces heavy-lock's exit trap, so its
   build lock stays until the next waiter reaps it; that is harmless.
2. **CI's toolchain:** before reporting, run what CI runs with the current stable in a separate target
   volume: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features`,
   `cargo nextest run --workspace`, `cargo test --doc` on the affected crates and `cargo run -p
   pumpkin-tester`, all with `RUSTFLAGS=-Dwarnings`. F-9 did it with a small script that sources
   CubeAttack's `scripts/rust-image.sh` and `scripts/heavy-lock.sh`, takes the build lock and calls
   `rust_docker_run` with `-e RUSTUP_TOOLCHAIN=<stable version> -e RUSTFLAGS=-Dwarnings` and its own target
   volume (`rustup toolchain install <version> --profile minimal -c rustfmt,clippy` inside). The
   container runs scripts with `sh -e`, so log each step's exit code inside an `if`. Remove the extra
   volume afterwards.
3. **The proto chunk loop** (CubeAttack `worldgen/fork/tools/proto_parity.py`, its proto-dump modules):
   copy `proto-dump/*.rs` into `crates/pumpkin-world/src/generation/proto_chunk_test/` and register them
   in its `mod.rs`, build the release test binary in the rust container under the build lock
   (`CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo test --release -p
   pumpkin-world --lib --no-run`, then copy `target/release/deps/pumpkin_world-<hash>` out of the target
   volume), and restore the worktree before anything is committed. The binary is a Linux build, so run
   `proto_parity.py` in a Linux container too (`python:3.13-slim-trixie`) with the CubeAttack clone, the
   verification data, the binary and an output folder mounted. The vanilla runs on the runner machine
   are junctions into another folder (below); mount the junctions' target at the path they resolve to
   inside Docker Desktop (`/mnt/host/d/...`). F-9's order models are in
   `D:\factory-work\_evidence\F-9\a2\proto_parity-waves-order.patch` (`waves2` is the order the
   scheduler builds in); land it in CubeAttack.
4. **The server verification:** build an image (`--image`), then in a CubeAttack checkout with the kept
   vanilla runs: `PUMPKIN_IMAGE=cubeattack/pumpkin-local PUMPKIN_TAG=<tag> WORLDGEN_ACCEPT_EULA=true
   WORLDGEN_VANILLA_JAR=<server-26.3.jar> bash scripts/worldgen-compare.sh verify --seeds <seeds>`, about
   30 minutes per seed, then `verify --report-only` over all four for one report. Check first that every
   vanilla run is kept (`python -X utf8 -P scripts/worldgen_compare.py kept data/worldgen/verify/<seed>/vanilla-<n>
   --suite worldgen/suite.toml --plan data/worldgen/verify/<seed>/plan.json --jar-sha1 <sha1>`), or the
   verification starts five hours of vanilla runs. Never run two verify processes in one checkout: they
   share `data/worldgen/run-pumpkin.sh` and `data/worldgen/config`. Without the census tool (a native
   build) the report lacks the large-area start census. Each area's `reasons` in the report's JSON say
   why it fails; count them per reason before choosing what to fix next.
5. **Seed-cracking check** (deployment rule 7, `docs/seed-cracking-check.md`): after every change to
   generation, as below.

### Where the data is on the runner machine

- Vanilla runs of the four seeds: `D:\factory-work\_scratch-F-5\root\data\worldgen\verify\<seed>\vanilla-1..3`,
  junctions into `D:\factory-work\_scratch-F-4\root\data\worldgen\verify\<seed>\`; the blocks report and
  the server jar under `D:\factory-work\_scratch-F-5\root\data\worldgen\reports` and `...\jars`.
- F-9's verification root (the same junctions, F-9's Pumpkin runs, the reports):
  `D:\factory-work\_scratch-F-9\cubeattack-main\data\worldgen\verify`; the JSON of the run above is
  `report-20261113T022632Z.json` there.
- F-9's evidence: `D:\factory-work\_evidence\F-9\a2` (`status-after-merge.md` has the full status with its
  sources, `seedcheck\` the seed-cracking check).
- These are scratch folders and can be cleaned up; ask the owner before relying on them. Without them,
  `scripts/worldgen-compare.sh verify --vanilla-only` makes new vanilla runs (about five hours, 8 GB).

## Seed-cracking check

CubeAttack's deployment rule 7: after a change to generation, the control world must crack, the secret
world must not, and the Nether bedrock must follow the derived seed. F-9 ran it on 2026-11-15 and 16 on
an image of its last code commit (1f0ef4d0b):

- **Offline** (`scripts/seedcrack_check.py` on worlds made by `scripts/worldgen-compare.sh generate
  pumpkin` with a new 64-bit seed from a CSPRNG, the overworld at radius 32 and the Nether at radius 12,
  once with `WORLDGEN_SECURE_GENERATION=false` and once with `true`):
  - `expected`: without the secret every part seed is the world seed and every structure set has
    vanilla's salt; with it every part seed, the hashed seed sent to clients and every set's salt
    derive from the secret.
  - `structures`, every start of the supported sets in the area (from the chunks' structure
    references): control 7 of 7 where the world seed puts them; secret 0 of 7 there and 7 of 7 where
    the derived salts put them.
  - `bedrock`, 841 full Nether chunks, 1,722,368 positions of the gradient layers: control: the world
    seed reproduces 100.00 %, a wrong seed 60.06 %; secret: the world seed 59.97 % (chance), the derived
    surface seed 100.00 %.
- **Live** (the play stack of a CubeAttack clone, seed `cubeattack-play` (654795844), the headless 26.3
  client with SeedCrackerX 2.16.2 and Cloth Config 26.3.159, `scripts/seedcrack_hunt.py` in spectator
  mode):
  - Control: the mod started lifting with 5 structures while the hunt was still circling the first
    desert pyramid (that pyramid and four structures it saw while the hunt teleported around it) and
    printed "Found structure seed [654795844]" and "Found world seed [654795844]" 18 seconds later. The
    igloo, the jungle temple and the swamp hut were located afterwards; all 4 located structures are
    where the world seed puts them.
  - Secret: 5 structures located (two desert pyramids, an igloo, a jungle temple, a swamp hut), 0 where
    the world seed puts them and 5 where the derived salts put them. The mod lifted with 5 and printed
    "Finished search with no results" at once.

Repeating it on the runner machine:

- Run the play stack from a scratch clone of CubeAttack, never from the main checkout: `play.sh start
  --fresh` replaces the play world.
- Build the image with `pumpkin-check.sh --image` and set `PUMPKIN_IMAGE=cubeattack/pumpkin-local
  PUMPKIN_TAG=<tag>`. Also set `PUMPKIN_API_SRC` to a checkout at the image's commit, because
  `build-ingot.sh` refuses Ingot plugins built against another plugin API revision. A clone of the
  worktree (`git clone --no-checkout`, then a checkout of the commit) stays put while the branch moves on.
- Download SeedCrackerX and Cloth Config for the client's Minecraft version yourself (the GitHub release,
  Modrinth) and compare their hashes with those pages.
- The play server stops itself after 15 minutes without a heartbeat from an MCP client; a script that
  drives the client touches a `data/play/heartbeat-<name>` file every 30 seconds instead.
- The client can be dropped on joining with "Network Protocol Error": Pumpkin ticks a joining player
  while plugins handle its spawn location event, before it sends the login packet, so a velocity packet
  can arrive first (a race outside generation, seen once in three joins). Check `connected` in the
  client's `/state` before hunting and reconnect with `POST /connect {"address": "server:25565"}`.
- F-9's scripts: `D:\factory-work\_scratch-F-9\seedcheck.sh` (offline) and `seedhunt2.sh` (live); the
  results: `D:\factory-work\_evidence\F-9\a2\seedcheck`.

## Open work, in order

Why the 105 areas fail (an area can have several reasons): fluid 23, item entities 15, terrain 13, ore
12, tree 12, vegetation 10, cats 9, dripstone 8, sculk sensor block entities 7, and per block name
deepslate 9, andesite 8, and clay, lava, gravel, diorite and copper ore 7 each.

1. **Split order from real differences in the decorated chunks.** In the order the scheduler models,
   227 of the 480 sampled decorated proto chunks still differ from vanilla (164 with the chunks two away
   decorated too). For each, find out whether some decoration order explains it (more orders and
   `--radius 2` in proto_parity) or a feature differs. Most failing areas fail on decoration (ore and
   stone blobs, trees, vegetation, dripstone, sculk, lush caves), and only this split says which of
   them code can fix. F-7 found one real difference this way (item 3). Start with the census areas,
   where the fork is furthest from vanilla's noise (seed 7853492499715517705's terrain and ore three
   times vanilla-3's, see "Where it stands").
2. **The decoration ring:** the fork decorates three rings past the full chunks (`chunk_state.rs`:
   full up to level 43, decorated at 44 to 46) where vanilla decorates one (level 34,
   `INITIALIZE_LIGHT`). Chunks next to an area generated earlier are then decorated in that earlier
   batch, where vanilla decorates them later. This may be what holds back the Nether portal area of seed
   1657324430, which overlaps the earlier batch's Nether area around 0 0 (565 differing blocks with
   f-5-worldgen's x-then-z order, 4,800 now, against a control of 568). Check what reducing the pyramid
   costs the lighting and spawn stages.
3. **The lush caves clay pool** vanilla never places (seed 1657324430, chunk (-59, 85)): the chunk's own
   `lush_caves_clay` places a pool with its 48th of 62 candidates where all three vanilla runs have moss
   on deepslate, and vanilla's blocks fit only "that candidate placed nothing", so something vanilla
   sees when the chunk is decorated differs from what the fork sees. CubeAttack
   `docs/worldgen-handoff.md` (section 3, "A lush caves clay pool vanilla never places") has the trace;
   the next step is an oracle (vanilla's decoration of that chunk in the probe container, or a Java
   agent logging the environment scans).
4. **Fluids** (32 areas fail on the fluid category or on lava or water by name, 9 of them on nothing
   else): find which come from generation (lakes, springs, the aquifer, lava a feature opens) and which
   from the settle ticks. F-7 counted fluids as runtime without tracing them per area.
5. **Structure pieces:** mineshaft rails, cobwebs and planks in the census areas of seeds
   -4172144997902289642 and 8095405736471581734 (rails 173 and 187 against a control of 14), where the
   floors under the differing rails are the same in both worlds, so the corridor's random diverges
   earlier: log the draws per piece in the fork and replay vanilla's with a probe. Seed
   7853492499715517705's fortress: 89 nether bricks against a control of 3 (CubeAttack next-steps item 8
   traced a bridge pillar through a cave where the fork had a huge crimson fungus).
6. **Saved structure starts:** the fork saves `structures.starts` empty, so a piece's state (a
   corridor's spawner, a temple's height) starts over after a restart, and vanilla does not know the
   fork's structures. Write and read vanilla's `starts` (`StructurePiece.createTag` per piece type).
7. **Runtime, not generation,** which a played world shows: item entities (15 areas, 8 on items
   alone; CubeAttack next-steps item 6 found most extra items older than the settle ticks, dropped
   during generation or in a chunk's first ticks), cats (9 areas), drowned (4), no bee ever leaves its
   hive (vanilla 120 bees summed over the areas, the fork 0; CubeAttack board ticket #41), no
   precipitation ticks (#43), no zombie nautiluses (vanilla 8, the fork 0; #44).
8. **Measurement and infrastructure:** CubeAttack's check image pins Rust 1.99.1 while CI uses stable;
   the AI test `avoid_entity::tests::creeper_starts_fleeing_a_spawned_ocelot_within_vanilla_retry_window`
   failed once in four full runs under load; the verification's own gaps (CubeAttack next-steps item
   12) and two it does not list: under the generation secret it checks the distribution of structures
   and dungeons only, not of caves or the other features, and the large-area start census needs the
   census tool.

Every change to generation also needs the seed-cracking check before players get a world from it, and
the next verification round goes into CubeAttack's `docs/worldgen-compare.md`.
