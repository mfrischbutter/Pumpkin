use super::*;
use crate::chunk_system::dag::Node;
use crate::chunk_system::dag::NodeKey;
use slotmap::Key;
use std::collections::BinaryHeap;
use std::collections::HashMap;

#[test]
fn ensure_dependency_chain_builds_multistage_chain() {
    let mut graph = DAG::default();
    let mut queue = BinaryHeap::new();
    let last_level: ChunkLevel = HashMapType::default();
    let last_high_priority: Vec<ChunkPos> = Vec::new();

    let chunk_pos = ChunkPos::new(0, 0);

    // Create a dependency node in the graph which will depend on the chain
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        ..Default::default()
    };

    // Build a chain up to Surface (Empty -> ... -> Surface)
    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &last_level,
        &last_high_priority,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    let start = (holder.current_stage as usize + 1).max(StagedChunkEnum::Empty as usize);
    let end = StagedChunkEnum::Surface as u8 as usize;

    // Ensure tasks were created, present in the DAG, and correctly chained
    for (idx, &key) in holder.tasks.iter().enumerate().take(end + 1).skip(start) {
        assert!(!key.is_null(), "task {idx} was not created");

        let node = graph.nodes.get(key).expect("graph missing node");

        if idx == start {
            assert_eq!(node.in_degree, 0, "Start task should have 0 in_degree");
        } else {
            assert_eq!(
                node.in_degree, 1,
                "Intermediate task {idx} should have in_degree of 1"
            );
        }
    }

    // The dependency node should have its in_degree incremented
    let dep_node = graph.nodes.get(dependency_task).unwrap();
    assert_eq!(dep_node.in_degree, 1);

    // The entry task should have been queued
    let queued = queue.pop().expect("queue should have entry task");
    assert_eq!(queued.node_key(), holder.tasks[start]);
}

#[test]
fn ensure_dependency_chain_resumes_partial_chain() {
    let mut graph = DAG::default();
    let mut queue = BinaryHeap::new();
    let last_level: ChunkLevel = HashMapType::default();
    let last_high_priority: Vec<ChunkPos> = Vec::new();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Biomes,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &last_level,
        &last_high_priority,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    // Dynamically calculate the next stage after Biomes instead of guessing
    let empty = StagedChunkEnum::Empty as usize;
    let start = (holder.current_stage as usize + 1).max(empty);

    let queued = queue.pop().expect("queue should have entry task");
    assert_eq!(
        queued.node_key(),
        holder.tasks[start],
        "Should resume directly from the next stage after Biomes"
    );

    let entry_node = graph.nodes.get(queued.node_key()).unwrap();
    assert_eq!(
        entry_node.in_degree, 0,
        "Resumed task should have 0 in_degree because previous stages are already done"
    );
}

#[test]
fn ensure_dependency_chain_does_nothing_if_already_met() {
    let mut graph = DAG::default();
    let mut queue = BinaryHeap::new();
    let last_level: ChunkLevel = HashMapType::default();
    let last_high_priority: Vec<ChunkPos> = Vec::new();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Full,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &last_level,
        &last_high_priority,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface, // Requesting a lower stage than it currently is
    );

    // Ensure the function returned early without creating any tasks or queueing anything
    for task in &holder.tasks {
        assert!(
            task.is_null(),
            "No tasks should be created if the stage requirement is already met"
        );
    }
    assert!(queue.is_empty(), "Nothing should be queued");
}

#[test]
fn ensure_dependency_chain_respects_occupied_lock() {
    let mut graph = DAG::default();
    let mut queue = BinaryHeap::new();
    let last_level: ChunkLevel = HashMapType::default();
    let last_high_priority: Vec<ChunkPos> = Vec::new();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    // Create an "occupy" node to simulate another thread/process currently working on this chunk
    let occupy_node = graph.nodes.insert(Node::new(
        ChunkPos::new(i32::MAX, i32::MAX),
        StagedChunkEnum::None,
    ));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        occupied: occupy_node,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &last_level,
        &last_high_priority,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    let start = StagedChunkEnum::Empty as usize;
    let entry_task = holder.tasks[start];

    let entry_node = graph.nodes.get(entry_task).unwrap();
    // The first task should depend on the occupy node finishing!
    assert_eq!(
        entry_node.in_degree, 1,
        "Entry task should be blocked by the occupy node"
    );

    // Therefore, the queue MUST be empty. It shouldn't fire until the occupy node drops.
    assert!(
        queue.is_empty(),
        "Task should not be queued because it is blocked by occupied status"
    );
}

#[test]
fn ensure_dependency_chain_early_return_skips_edge() {
    let mut graph = DAG::default();
    let mut queue = BinaryHeap::new();
    let last_level = HashMapType::default();
    let last_high_priority = Vec::new();

    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 1), StagedChunkEnum::Surface));

    // Create a holder that is already at the required stage (Features > Surface)
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Features,
        target_stage: StagedChunkEnum::Features,
        ..Default::default()
    };

    // Require 'Empty', but holder is already at 'Features'
    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &last_level,
        &last_high_priority,
        dependency_task,
        ChunkPos::new(0, 0),
        &mut holder,
        StagedChunkEnum::Empty,
    );

    let dep_node = graph.nodes.get(dependency_task).unwrap();

    // Because of the early return, the dependency_task should not have its in_degree increased
    assert_eq!(
        dep_node.in_degree, 0,
        "Dependency task should not be blocked if the neighbor is already past the required stage"
    );
}

#[test]
fn completed_proto_stage_drops_all_satisfied_tasks() {
    let mut graph = DAG::default();
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        target_stage: StagedChunkEnum::StructureStart,
        ..Default::default()
    };

    for (stage, task) in holder
        .tasks
        .iter_mut()
        .enumerate()
        .take(StagedChunkEnum::StructureStart as usize + 1)
        .skip(StagedChunkEnum::Empty as usize)
    {
        *task = graph.nodes.insert(Node::new(
            ChunkPos::new(0, 0),
            StagedChunkEnum::from(stage as u8),
        ));
    }

    for stage in StagedChunkEnum::Empty as usize..StagedChunkEnum::StructureStart as usize {
        graph.add_edge(holder.tasks[stage], holder.tasks[stage + 1]);
    }

    let returned_stage = StagedChunkEnum::Biomes as usize;
    for task_idx in
        (holder.current_stage as usize + 1)..=(returned_stage).min(holder.tasks.len() - 1)
    {
        if !holder.tasks[task_idx].is_null() {
            if let Some(old) = graph.nodes.remove(holder.tasks[task_idx]) {
                let mut edge = old.edge;
                while !edge.is_null() {
                    let cur = graph.edges.remove(edge).unwrap();
                    if let Some(node) = graph.nodes.get_mut(cur.to) {
                        node.in_degree -= 1;
                    }
                    edge = cur.next;
                }
            }
            holder.tasks[task_idx] = NodeKey::null();
        }
    }

    assert!(holder.tasks[StagedChunkEnum::Empty as usize].is_null());
    assert!(holder.tasks[StagedChunkEnum::Biomes as usize].is_null());
    assert!(!holder.tasks[StagedChunkEnum::StructureStart as usize].is_null());
    assert!(
        graph
            .nodes
            .get(holder.tasks[StagedChunkEnum::StructureStart as usize])
            .is_some()
    );
}

#[test]
fn cancellation_path_decrements_in_degree() {
    let mut graph = DAG::default();

    // Create a task that is waiting (in_degree = 1)
    let waiting_task_key = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Surface));
    let waiting_node = graph.nodes.get_mut(waiting_task_key).unwrap();
    waiting_node.in_degree = 1;

    // Create the occupy node that the task is waiting on
    let occupy_key = graph.nodes.insert(Node::new(
        ChunkPos::new(i32::MAX, i32::MAX),
        StagedChunkEnum::None,
    ));
    graph.add_edge(occupy_key, waiting_task_key);

    // Set up the chunk map to simulate an in-flight task
    let mut chunk_map = HashMap::new();
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Empty,
        target_stage: StagedChunkEnum::Surface,
        occupied: occupy_key,
        ..Default::default()
    };
    holder.tasks[StagedChunkEnum::Surface as usize] = waiting_task_key;
    chunk_map.insert(ChunkPos::new(0, 0), holder);

    // Simulate the cancellation logic from `work()`
    let mut nodes_to_drop = Vec::new();
    for holder in chunk_map.values_mut() {
        for task in &mut holder.tasks {
            if !task.is_null() {
                nodes_to_drop.push(*task);
                *task = NodeKey::null();
            }
        }
        if !holder.occupied.is_null() {
            nodes_to_drop.push(holder.occupied);
            holder.occupied = NodeKey::null();
        }
    }

    // Drop nodes using the proper graph edge traversal
    for node_key in nodes_to_drop {
        // Simulating the self.drop_node logic inside the test
        if let Some(old) = graph.nodes.remove(node_key) {
            let mut edge = old.edge;
            while !edge.is_null() {
                let cur = graph.edges.remove(edge).unwrap();
                if let Some(node) = graph.nodes.get_mut(cur.to) {
                    node.in_degree -= 1;
                }
                edge = cur.next;
            }
        }
    }

    // Assert that the waiting task was properly cleaned up
    assert!(
        graph.nodes.get(waiting_task_key).is_none(),
        "The waiting task should have been dropped during cancellation"
    );
}

#[test]
fn dag_drop_edge_chain_removes_all_edges() {
    let mut graph = DAG::default();
    let node1 = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Empty));
    let node2 = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 0), StagedChunkEnum::Empty));

    let edge1 = graph.edges.insert(crate::chunk_system::dag::Edge::new(
        node1,
        crate::chunk_system::dag::EdgeKey::null(),
    ));
    let edge2 = graph
        .edges
        .insert(crate::chunk_system::dag::Edge::new(node2, edge1));

    assert_eq!(graph.edges.len(), 2);
    graph.drop_edge_chain(edge2);
    assert_eq!(graph.edges.len(), 0);
}

#[test]
fn dag_prune_edge_chain_removes_dead_target_edges() {
    let mut graph = DAG::default();
    let node_alive = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Empty));
    let node_dead = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 0), StagedChunkEnum::Empty));

    let edge_alive = graph.edges.insert(crate::chunk_system::dag::Edge::new(
        node_alive,
        crate::chunk_system::dag::EdgeKey::null(),
    ));
    let edge_dead = graph
        .edges
        .insert(crate::chunk_system::dag::Edge::new(node_dead, edge_alive));

    // Remove node_dead from graph.nodes to simulate completed/dropped task
    graph.nodes.remove(node_dead);

    let mut head = edge_dead;
    let has_valid_task = graph.prune_edge_chain(&mut head);

    assert!(has_valid_task);
    assert_eq!(head, edge_alive);
    assert_eq!(graph.edges.len(), 1);
    assert!(graph.edges.contains_key(edge_alive));
    assert!(!graph.edges.contains_key(edge_dead));

    // Now remove node_alive as well
    graph.nodes.remove(node_alive);
    let has_valid_task = graph.prune_edge_chain(&mut head);
    assert!(!has_valid_task);
    assert!(head.is_null());
    assert_eq!(graph.edges.len(), 0);
}

#[test]
fn cancelled_out_of_range_task_clears_holder_task_slot() {
    let mut graph = DAG::default();
    let mut chunk_map = HashMap::new();
    let pos = ChunkPos::new(0, 0);

    let stage = StagedChunkEnum::Biomes;
    let node_key = graph.nodes.insert(Node::new(pos, stage));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Empty,
        target_stage: StagedChunkEnum::None, // Chunk was downgraded / moved out of range
        dependency_stage: StagedChunkEnum::None,
        ..Default::default()
    };
    holder.tasks[stage as usize] = node_key;
    chunk_map.insert(pos, holder);

    // Simulate task processing loop cancellation when node.stage > effective_target
    let node = graph.nodes.get(node_key).unwrap().clone();
    let effective_target = chunk_map.get(&node.pos).map_or(StagedChunkEnum::None, |h| {
        h.target_stage.max(h.dependency_stage)
    });

    assert!(node.stage > effective_target);
    if let Some(holder) = chunk_map.get_mut(&node.pos) {
        let task_slot = &mut holder.tasks[node.stage as usize];
        if *task_slot == node_key {
            *task_slot = NodeKey::null();
        }
    }
    graph.fast_drop_node(node_key);

    let holder = chunk_map.get(&pos).unwrap();
    assert!(
        holder.tasks[stage as usize].is_null(),
        "Task slot must be null after cancellation"
    );
    assert!(
        holder.tasks.iter().all(Key::is_null),
        "All task slots must be null when idle"
    );
}

#[test]
fn center_chunk_pyramid_prioritized_ahead_of_outer_rings() {
    let mut last_level: ChunkLevel = HashMapType::default();
    let base_center_level = 32i8;
    for dx in -10i32..=10 {
        for dz in -10i32..=10 {
            let dist = dx.abs().max(dz.abs()) as i8;
            last_level.insert(ChunkPos::new(dx, dz), base_center_level + dist);
        }
    }
    let high_priority = Vec::new();

    let center_full = GenerationSchedule::calc_priority(
        &last_level,
        &high_priority,
        ChunkPos::new(0, 0),
        StagedChunkEnum::Full,
    );
    let pyramid_edge_empty = GenerationSchedule::calc_priority(
        &last_level,
        &high_priority,
        ChunkPos::new(5, 5),
        StagedChunkEnum::Empty,
    );
    let ring1_full = GenerationSchedule::calc_priority(
        &last_level,
        &high_priority,
        ChunkPos::new(1, 0),
        StagedChunkEnum::Full,
    );
    let outer_ring_empty = GenerationSchedule::calc_priority(
        &last_level,
        &high_priority,
        ChunkPos::new(10, 0),
        StagedChunkEnum::Empty,
    );

    // TaskHeapNode reverses Ord so smaller TaskPriority pops first from BinaryHeap.
    assert!(
        center_full < pyramid_edge_empty,
        "Higher stage (Full) in the same dependency wave must run before Empty"
    );
    assert!(
        pyramid_edge_empty < ring1_full,
        "Center chunk (0,0) dependency pyramid must precede ring-1 Full"
    );
    assert!(
        ring1_full < outer_ring_empty,
        "Inner ring Full chunks must not be preempted by outer-ring Empty tasks"
    );
}

#[test]
fn features_task_waits_for_higher_priority_overlapping_features_neighbor() {
    let mut graph = DAG::default();
    let mut chunk_map = HashMap::new();

    let earlier_pos = ChunkPos::new(-1, -1);
    let later_pos = ChunkPos::new(0, 0);

    let earlier_key = graph
        .nodes
        .insert(Node::new(earlier_pos, StagedChunkEnum::Features));
    let later_key = graph
        .nodes
        .insert(Node::new(later_pos, StagedChunkEnum::Features));

    let mut earlier_holder = ChunkHolder {
        current_stage: StagedChunkEnum::Carvers,
        target_stage: StagedChunkEnum::Full,
        ..Default::default()
    };
    earlier_holder.tasks[StagedChunkEnum::Features as usize] = earlier_key;
    chunk_map.insert(earlier_pos, earlier_holder);

    let mut later_holder = ChunkHolder {
        current_stage: StagedChunkEnum::Carvers,
        target_stage: StagedChunkEnum::Full,
        ..Default::default()
    };
    later_holder.tasks[StagedChunkEnum::Features as usize] = later_key;
    chunk_map.insert(later_pos, later_holder);

    let last_level = ChunkLevel::default();
    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        earlier_pos,
    ));
    assert!(!GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        later_pos,
    ));

    chunk_map.get_mut(&earlier_pos).unwrap().current_stage = StagedChunkEnum::Features;

    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        later_pos,
    ));
}

/// The levels `ChunkLoading` gives the chunks around a forced rectangle: the forced level inside,
/// one more per chunk of distance, until the unloaded level.
fn forced_area_levels(x0: i32, x1: i32, z0: i32, z1: i32) -> ChunkLevel {
    let reach = i32::from(ChunkLoading::MAX_LEVEL - ChunkLoading::FORCED_CHUNK_LEVEL - 1);
    let mut levels = ChunkLevel::default();
    for x in (x0 - reach)..=(x1 + reach) {
        for z in (z0 - reach)..=(z1 + reach) {
            let distance = (x0 - x).max(x - x1).max(z0 - z).max(z - z1).max(0);
            levels.insert(
                ChunkPos::new(x, z),
                ChunkLoading::FORCED_CHUNK_LEVEL + distance as i8,
            );
        }
    }
    levels
}

fn waiting_for_features() -> ChunkHolder {
    ChunkHolder {
        current_stage: StagedChunkEnum::Carvers,
        target_stage: StagedChunkEnum::Full,
        ..Default::default()
    }
}

#[test]
fn features_wait_for_the_overlapping_neighbour_nearer_the_tickets() {
    let last_level = forced_area_levels(0, 15, 0, 15);
    let nearer = ChunkPos::new(-2, 5);
    let farther = ChunkPos::new(-3, 5);
    let mut chunk_map = HashMap::new();
    chunk_map.insert(nearer, waiting_for_features());
    chunk_map.insert(farther, waiting_for_features());

    // x, then z alone would decorate the farther chunk first.
    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        nearer,
    ));
    assert!(!GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        farther,
    ));

    chunk_map.get_mut(&nearer).unwrap().current_stage = StagedChunkEnum::Features;

    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        farther,
    ));
}

/// Vanilla's forced chunks generate the terrain two chunks around them and decorate their 3x3 x
/// first, so the chunks on a forced area's edge and just outside it come in x, then z like the
/// inside, not after it.
#[test]
fn features_at_a_forced_edge_come_in_x_then_z() {
    let last_level = forced_area_levels(0, 15, 0, 15);
    let outside = ChunkPos::new(-1, 5);
    let edge = ChunkPos::new(0, 5);
    let inside = ChunkPos::new(1, 5);
    let mut chunk_map = HashMap::new();
    for pos in [outside, edge, inside] {
        chunk_map.insert(pos, waiting_for_features());
    }

    for (pos, ready) in [(outside, true), (edge, false), (inside, false)] {
        assert_eq!(
            GenerationSchedule::is_features_order_ready(
                &chunk_map,
                &last_level,
                &mut HashMapType::default(),
                pos,
            ),
            ready,
            "{pos:?}"
        );
    }

    chunk_map.get_mut(&outside).unwrap().current_stage = StagedChunkEnum::Features;

    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        edge,
    ));
}

/// Vanilla decorated (-89, -131) before the corner (-89, -132) of the ring around seed
/// -4172144997902289642's village area: their two geodes overlap, and only that order gives
/// vanilla's blocks.
#[test]
fn features_follow_vanillas_order_at_a_ring_corner() {
    let last_level = forced_area_levels(-86, -70, -129, -113);
    let first = ChunkPos::new(-89, -131);
    let corner = ChunkPos::new(-89, -132);
    let mut chunk_map = HashMap::new();
    chunk_map.insert(first, waiting_for_features());
    chunk_map.insert(corner, waiting_for_features());

    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        first,
    ));
    assert!(!GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        corner,
    ));

    chunk_map.get_mut(&first).unwrap().current_stage = StagedChunkEnum::Features;

    assert!(GenerationSchedule::is_features_order_ready(
        &chunk_map,
        &last_level,
        &mut HashMapType::default(),
        corner,
    ));
}
