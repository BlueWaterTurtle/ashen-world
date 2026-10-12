use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// Integer grid cell (world cell coordinates, 1 cell = 1 meter).
pub type Cell = (i32, i32);

/// Reusable A* scratch storage. Keeps the open list and per-cell search state
/// between searches so repeated queries don't reallocate.
///
/// Heap ordering is explicit and deterministic: lowest `f`, then lowest `g`,
/// then lowest cell (x, then z). Neighbours are expanded in a fixed order.
#[derive(Debug, Default)]
pub struct Pathfinder {
    open: BinaryHeap<Reverse<(i32, i32, Cell)>>,
    /// Best known cost and predecessor for each visited cell.
    nodes: HashMap<Cell, (i32, Cell)>,
}

impl Pathfinder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Find a shortest 8-connected path from `start` to `goal` using A*.
    ///
    /// `passable` decides whether a cell can be entered. Diagonal moves are only
    /// allowed when both adjacent orthogonal cells are passable. `max_expansions`
    /// bounds the search so it can't stall a tick. Returns the cells including both
    /// endpoints, or `None` if blocked or the budget is exhausted.
    pub fn find_path(
        &mut self,
        start: Cell,
        goal: Cell,
        max_expansions: usize,
        mut passable: impl FnMut(Cell) -> bool,
    ) -> Option<Vec<Cell>> {
        if !passable(start) || !passable(goal) {
            return None;
        }
        if start == goal {
            return Some(vec![start]);
        }
        // Adjacent goal: the direct step is the unique optimal path.
        let (adx, adz) = ((goal.0 - start.0).abs(), (goal.1 - start.1).abs());
        if adx <= 1
            && adz <= 1
            && (adx == 0 || adz == 0 || {
                passable((goal.0, start.1)) && passable((start.0, goal.1))
            })
        {
            return Some(vec![start, goal]);
        }

        // Costs are in tenths: 10 orthogonal, 14 diagonal.
        let h = |c: Cell| {
            let (dx, dz) = ((c.0 - goal.0).abs(), (c.1 - goal.1).abs());
            10 * (dx + dz) - 6 * dx.min(dz)
        };
        self.open.clear();
        self.nodes.clear();
        self.nodes.insert(start, (0, start));
        self.open.push(Reverse((h(start), 0, start)));
        let mut expansions = 0;
        while let Some(Reverse((_, cost, cur))) = self.open.pop() {
            if cur == goal {
                let mut path = vec![cur];
                let mut c = cur;
                while c != start {
                    c = self.nodes[&c].1;
                    path.push(c);
                }
                path.reverse();
                return Some(path);
            }
            if cost > self.nodes[&cur].0 {
                continue;
            }
            expansions += 1;
            if expansions > max_expansions {
                return None;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let n = (cur.0 + dx, cur.1 + dz);
                    if !passable(n) {
                        continue;
                    }
                    let diag = dx != 0 && dz != 0;
                    if diag && (!passable((cur.0 + dx, cur.1)) || !passable((cur.0, cur.1 + dz))) {
                        continue;
                    }
                    let nc = cost + if diag { 14 } else { 10 };
                    if self.nodes.get(&n).is_none_or(|&(old, _)| nc < old) {
                        self.nodes.insert(n, (nc, cur));
                        self.open.push(Reverse((nc + h(n), nc, n)));
                    }
                }
            }
        }
        None
    }
}

/// Convenience wrapper around [`Pathfinder::find_path`] using a throwaway pathfinder.
pub fn find_path(
    start: Cell,
    goal: Cell,
    max_expansions: usize,
    passable: impl FnMut(Cell) -> bool,
) -> Option<Vec<Cell>> {
    Pathfinder::new().find_path(start, goal, max_expansions, passable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_and_diagonal_paths() {
        let p = find_path((0, 0), (5, 0), 1000, |_| true).unwrap();
        assert_eq!(p.len(), 6);
        let p = find_path((0, 0), (3, 3), 1000, |_| true).unwrap();
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn routes_around_wall() {
        let wall = |c: Cell| !(c.0 == 2 && (-3..=3).contains(&c.1));
        let p = find_path((0, 0), (4, 0), 10_000, wall).unwrap();
        assert!(p.iter().all(|&c| wall(c)));
        assert_eq!((p[0], *p.last().unwrap()), ((0, 0), (4, 0)));
        assert!(p.len() > 5);
    }

    #[test]
    fn unreachable_or_over_budget_is_none() {
        let boxed = |c: Cell| c == (0, 0) || c.0.abs() > 1 || c.1.abs() > 1 || c == (5, 5);
        assert!(find_path((0, 0), (5, 5), 10_000, boxed).is_none());
        assert!(find_path((0, 0), (500, 500), 10, |_| true).is_none());
    }

    /// Original per-call-allocating implementation, kept as a reference.
    fn reference_find_path(
        start: Cell,
        goal: Cell,
        max_expansions: usize,
        mut passable: impl FnMut(Cell) -> bool,
    ) -> Option<Vec<Cell>> {
        if !passable(start) || !passable(goal) {
            return None;
        }
        let h = |c: Cell| {
            let (dx, dz) = ((c.0 - goal.0).abs(), (c.1 - goal.1).abs());
            10 * (dx + dz) - 6 * dx.min(dz)
        };
        let mut open = BinaryHeap::new();
        let mut g: HashMap<Cell, i32> = HashMap::new();
        let mut came: HashMap<Cell, Cell> = HashMap::new();
        g.insert(start, 0);
        open.push(Reverse((h(start), 0, start)));
        let mut expansions = 0;
        while let Some(Reverse((_, cost, cur))) = open.pop() {
            if cur == goal {
                let mut path = vec![cur];
                let mut c = cur;
                while let Some(&p) = came.get(&c) {
                    path.push(p);
                    c = p;
                }
                path.reverse();
                return Some(path);
            }
            if cost > g[&cur] {
                continue;
            }
            expansions += 1;
            if expansions > max_expansions {
                return None;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let n = (cur.0 + dx, cur.1 + dz);
                    if !passable(n) {
                        continue;
                    }
                    let diag = dx != 0 && dz != 0;
                    if diag && (!passable((cur.0 + dx, cur.1)) || !passable((cur.0, cur.1 + dz))) {
                        continue;
                    }
                    let nc = cost + if diag { 14 } else { 10 };
                    if g.get(&n).is_none_or(|&old| nc < old) {
                        g.insert(n, nc);
                        came.insert(n, cur);
                        open.push(Reverse((nc + h(n), nc, n)));
                    }
                }
            }
        }
        None
    }

    #[test]
    fn reusable_pathfinder_matches_reference() {
        type Grid = Box<dyn Fn(Cell) -> bool>;
        let grids: Vec<(Grid, usize)> = vec![
            (Box::new(|_| true), 10_000),
            (Box::new(|c| !(c.0 == 2 && (-3..=3).contains(&c.1))), 10_000),
            (
                Box::new(|c| c == (0, 0) || c.0.abs() > 1 || c.1.abs() > 1 || c == (5, 5)),
                10_000,
            ),
            (Box::new(|_| true), 10),
            (
                Box::new(|c| (c.0 * 7 + c.1 * 13).rem_euclid(5) != 0),
                10_000,
            ),
        ];
        let goals = [
            (0, 0),
            (1, 0),
            (1, 1),
            (-1, 1),
            (4, 0),
            (5, 5),
            (-9, 7),
            (30, -20),
        ];
        let mut pf = Pathfinder::new();
        for (grid, budget) in &grids {
            for &start in &[(0, 0), (1, 1), (-3, 2)] {
                for &goal in &goals {
                    let got = pf.find_path(start, goal, *budget, grid);
                    let want = reference_find_path(start, goal, *budget, grid);
                    assert_eq!(got, want, "start {start:?} goal {goal:?} budget {budget}");
                }
            }
        }
    }

    #[test]
    fn trivial_cases_skip_search() {
        let mut pf = Pathfinder::new();
        assert_eq!(
            pf.find_path((3, 3), (3, 3), 0, |_| true),
            Some(vec![(3, 3)])
        );
        assert_eq!(
            pf.find_path((3, 3), (4, 3), 0, |_| true),
            Some(vec![(3, 3), (4, 3)])
        );
        assert_eq!(
            pf.find_path((3, 3), (4, 4), 0, |_| true),
            Some(vec![(3, 3), (4, 4)])
        );
        // Diagonal with a blocked corner must still go around.
        let p = pf.find_path((0, 0), (1, 1), 100, |c| c != (1, 0)).unwrap();
        assert!(p.len() > 2);
    }
}
