use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// Integer grid cell (world cell coordinates, 1 cell = 1 meter).
pub type Cell = (i32, i32);

/// Find a shortest 8-connected path from `start` to `goal` using A*.
///
/// `passable` decides whether a cell can be entered. Diagonal moves are only
/// allowed when both adjacent orthogonal cells are passable. `max_expansions`
/// bounds the search so it can't stall a tick. Returns the cells including both
/// endpoints, or `None` if blocked or the budget is exhausted.
pub fn find_path(
    start: Cell,
    goal: Cell,
    max_expansions: usize,
    passable: impl Fn(Cell) -> bool,
) -> Option<Vec<Cell>> {
    if !passable(start) || !passable(goal) {
        return None;
    }
    // Costs are in tenths: 10 orthogonal, 14 diagonal.
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
                if g.get(&n).map_or(true, |&old| nc < old) {
                    g.insert(n, nc);
                    came.insert(n, cur);
                    open.push(Reverse((nc + h(n), nc, n)));
                }
            }
        }
    }
    None
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
}
