//! The dragon's 24 flight nodes and the path search over them (vanilla `EnderDragon.findPath`).

use pumpkin_util::math::vector3::Vector3;
use std::f32::consts::PI;

pub const NODE_COUNT: usize = 24;
/// The nodes from here on are the inner rings, all a dragon without crystals may use.
pub const INNER_NODES_START: usize = 12;

const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057,
    3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712,
    8257536,
];

pub type Nodes = [Vector3<i32>; NODE_COUNT];

/// A path being flown, vanilla `Path` as the dragon uses it.
#[derive(Clone, Debug)]
pub struct DragonPath {
    nodes: Vec<Vector3<i32>>,
    next: usize,
}

impl DragonPath {
    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.next >= self.nodes.len()
    }

    pub const fn advance(&mut self) {
        self.next += 1;
    }

    pub const fn finish(&mut self) {
        self.next = self.nodes.len();
    }

    #[must_use]
    pub fn next_pos(&self) -> Option<Vector3<i32>> {
        self.nodes.get(self.next).copied()
    }
}

/// The ring position of a node, without its height: 12 at 60 blocks, 8 at 40 and 4 at 20.
fn node_ring_pos(index: usize) -> (i32, i32, i32) {
    let (radius, step, offset, height) = match index {
        0..=11 => (60.0f32, PI / 12.0, 0, 5),
        12..=19 => (40.0, PI / 8.0, 12, 15),
        _ => (20.0, PI / 4.0, 20, 5),
    };
    let angle = 2.0 * (-PI + step * (index - offset) as f32);
    (
        (radius * angle.cos()).floor() as i32,
        height,
        (radius * angle.sin()).floor() as i32,
    )
}

/// Builds the nodes, each at least at height 73 and above what stands there.
pub fn build_nodes(height_at: impl Fn(i32, i32) -> i32) -> Nodes {
    std::array::from_fn(|index| {
        let (x, height, z) = node_ring_pos(index);
        Vector3::new(x, 73.max(height_at(x, z) + height), z)
    })
}

/// Vanilla `findClosestNode`: within 100 blocks, else the first node allowed.
#[must_use]
pub fn closest_node(nodes: &Nodes, pos: Vector3<f64>, first_allowed: usize) -> usize {
    let at = Vector3::new(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    );
    let mut closest = 10000.0;
    let mut index = 0;
    for (i, node) in nodes.iter().enumerate().skip(first_allowed) {
        let dist = dist_sq(*node, at);
        if dist < closest {
            closest = dist;
            index = i;
        }
    }
    index
}

fn dist_sq(a: Vector3<i32>, b: Vector3<i32>) -> f32 {
    let (x, y, z) = ((a.x - b.x) as f32, (a.y - b.y) as f32, (a.z - b.z) as f32);
    x * x + y * y + z * z
}

fn dist(a: Vector3<i32>, b: Vector3<i32>) -> f32 {
    dist_sq(a, b).sqrt()
}

/// Vanilla `findPath`: A* over the nodes from `first_allowed` on.
///
/// `final_node` is flown after them. A path that cannot be finished ends at the closest node
/// reached; when no other node was reached there is none.
#[must_use]
pub fn find_path(
    nodes: &Nodes,
    start: usize,
    end: usize,
    final_node: Option<Vector3<i32>>,
    first_allowed: usize,
) -> Option<DragonPath> {
    let to = nodes[end];
    let mut g = [0.0f32; NODE_COUNT];
    let mut f = [0.0f32; NODE_COUNT];
    let mut came_from = [None; NODE_COUNT];
    let mut closed = [false; NODE_COUNT];
    let mut open = [false; NODE_COUNT];
    f[start] = dist(nodes[start], to);
    open[start] = true;
    let mut closest = start;

    while let Some(current) = (0..NODE_COUNT)
        .filter(|&i| open[i])
        .min_by(|&a, &b| f[a].total_cmp(&f[b]))
    {
        if current == end {
            return Some(reconstruct(nodes, &came_from, end, final_node));
        }
        open[current] = false;
        closed[current] = true;
        if dist(nodes[current], to) < dist(nodes[closest], to) {
            closest = current;
        }
        for next in first_allowed..NODE_COUNT {
            if NODE_ADJACENCY[current] & (1 << next) == 0 || closed[next] {
                continue;
            }
            let tentative = g[current] + dist(nodes[current], nodes[next]);
            if !open[next] || tentative < g[next] {
                came_from[next] = Some(current);
                g[next] = tentative;
                f[next] = tentative + dist(nodes[next], to);
                open[next] = true;
            }
        }
    }

    (closest != start).then(|| reconstruct(nodes, &came_from, closest, final_node))
}

fn reconstruct(
    nodes: &Nodes,
    came_from: &[Option<usize>; NODE_COUNT],
    end: usize,
    final_node: Option<Vector3<i32>>,
) -> DragonPath {
    let mut path = vec![nodes[end]];
    let mut current = end;
    while let Some(previous) = came_from[current] {
        path.push(nodes[previous]);
        current = previous;
    }
    path.reverse();
    path.extend(final_node);
    DragonPath {
        nodes: path,
        next: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_nodes() -> Nodes {
        build_nodes(|_, _| 64)
    }

    #[test]
    fn nodes_lie_on_their_rings_at_vanilla_heights() {
        let nodes = flat_nodes();
        assert_eq!(nodes[0].y, 73, "the outer ring is at the minimum of 73");
        assert_eq!(nodes[12].y, 79, "the middle ring is 15 above the ground");
        assert_eq!(nodes[20].y, 73, "the inner ring is at the minimum of 73");
        let radius = |n: Vector3<i32>| f64::from(n.x).hypot(f64::from(n.z));
        assert!((radius(nodes[3]) - 60.0).abs() < 2.0);
        assert!((radius(nodes[15]) - 40.0).abs() < 2.0);
        assert!((radius(nodes[22]) - 20.0).abs() < 2.0);
    }

    #[test]
    fn a_path_starts_at_its_start_and_ends_with_the_final_node() {
        let nodes = flat_nodes();
        let podium = Vector3::new(0, 65, 0);
        let mut path = find_path(&nodes, 0, 6, Some(podium), 0).expect("the nodes are connected");
        assert_eq!(path.next_pos(), Some(nodes[0]));
        let mut last = None;
        while let Some(pos) = path.next_pos() {
            last = Some(pos);
            path.advance();
        }
        assert!(path.is_done());
        assert_eq!(last, Some(podium));
    }

    #[test]
    fn a_path_to_its_own_start_is_that_node_alone() {
        let nodes = flat_nodes();
        let mut path = find_path(&nodes, 5, 5, None, 0).expect("it is already there");
        path.advance();
        assert!(path.is_done());
    }

    #[test]
    fn without_crystals_paths_keep_to_the_inner_rings() {
        let nodes = flat_nodes();
        let mut path = find_path(&nodes, 12, 16, None, INNER_NODES_START).expect("connected");
        while let Some(pos) = path.next_pos() {
            assert!(
                nodes[INNER_NODES_START..].contains(&pos),
                "{pos:?} is on the outer ring"
            );
            path.advance();
        }
    }

    #[test]
    fn the_closest_node_only_looks_at_what_is_allowed() {
        let nodes = flat_nodes();
        let at = |i: usize| Vector3::new(f64::from(nodes[i].x), 73.0, f64::from(nodes[i].z));
        assert_eq!(closest_node(&nodes, at(3), 0), 3);
        assert!(closest_node(&nodes, at(3), INNER_NODES_START) >= INNER_NODES_START);
    }
}
