//! Commit graph lane layout.
//!
//! Commits arrive in topological order (children before parents). Each lane
//! holds the oid it is waiting for; a commit takes the first lane waiting for
//! it (or a free one), merges every other lane waiting for it into its own,
//! and hands its parents down: the first parent keeps the lane (duplicates
//! converge when the parent is reached), others reuse
//! a lane already waiting for them or open a new one. Lanes are never
//! compacted, so lines that pass through a row stay perfectly vertical.

use git2::Oid;
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    /// From the top edge of the row to the node.
    Top,
    /// From the node to the bottom edge of the row.
    Bottom,
    /// Straight through the row.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub from: u16,
    pub to: u16,
    pub half: Half,
    pub color: u8,
}

#[derive(Clone, Debug, Default)]
pub struct GraphRow {
    pub col: u16,
    pub color: u8,
    pub lines: SmallVec<[Line; 6]>,
}

pub struct Layout {
    pub rows: Vec<GraphRow>,
    /// Widest the graph gets, in lanes.
    pub width: u16,
}

pub fn layout<'a>(commits: impl IntoIterator<Item = (Oid, &'a [Oid])>) -> Layout {
    let mut lanes: Vec<Option<(Oid, u8)>> = Vec::new();
    let mut next_color = 0u8;
    let mut rows = Vec::new();
    let mut width = 1u16;

    for (oid, parents) in commits {
        let mut row = GraphRow::default();

        // Lanes waiting for this commit converge on it.
        let waiting: SmallVec<[usize; 4]> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| matches!(l, Some((o, _)) if *o == oid).then_some(i))
            .collect();

        let (col, color) = match waiting.first() {
            Some(&i) => (i, lanes[i].unwrap().1),
            None => {
                let c = next_color;
                next_color = next_color.wrapping_add(1);
                let i = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                    lanes.push(None);
                    lanes.len() - 1
                });
                (i, c)
            }
        };
        row.col = col as u16;
        row.color = color;

        for &i in &waiting {
            let c = lanes[i].unwrap().1;
            row.lines.push(Line { from: i as u16, to: col as u16, half: Half::Top, color: c });
            lanes[i] = None;
        }

        // Everything else passes straight through.
        for (i, lane) in lanes.iter().enumerate() {
            if let Some((_, c)) = lane {
                row.lines.push(Line { from: i as u16, to: i as u16, half: Half::Full, color: *c });
            }
        }

        for (pi, &p) in parents.iter().enumerate() {
            // The first parent always inherits this lane, even when another
            // lane already waits for it; the two converge at the parent.
            let existing = if pi == 0 {
                None
            } else {
                lanes.iter().position(|l| matches!(l, Some((o, _)) if *o == p))
            };
            let (target, c) = match existing {
                Some(i) => (i, lanes[i].unwrap().1),
                None if pi == 0 => {
                    lanes[col] = Some((p, color));
                    (col, color)
                }
                None => {
                    let c = next_color;
                    next_color = next_color.wrapping_add(1);
                    let i = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                        lanes.push(None);
                        lanes.len() - 1
                    });
                    lanes[i] = Some((p, c));
                    (i, c)
                }
            };
            // A merge line takes the colour of the branch being merged in.
            let line_color = if pi == 0 { color } else { c };
            row.lines.push(Line {
                from: col as u16,
                to: target as u16,
                half: Half::Bottom,
                color: line_color,
            });
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        width = width.max(lanes.len() as u16).max(col as u16 + 1);
        rows.push(row);
    }
    Layout { rows, width }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes(&[n; 20]).unwrap()
    }

    #[test]
    fn linear_history_is_one_lane() {
        let (a, b, c) = (oid(1), oid(2), oid(3));
        let commits = [(c, vec![b]), (b, vec![a]), (a, vec![])];
        let l = layout(commits.iter().map(|(o, p)| (*o, p.as_slice())));
        assert_eq!(l.width, 1);
        assert!(l.rows.iter().all(|r| r.col == 0));
        assert_eq!(l.rows[0].lines.len(), 1); // no incoming line on the tip
        assert!(l.rows[2].lines.iter().all(|x| x.half == Half::Top));
    }

    #[test]
    fn merge_opens_and_closes_a_lane() {
        // m merges f into d; f and d both come from a.
        let (a, d, f, m) = (oid(1), oid(2), oid(3), oid(4));
        let commits = [(m, vec![d, f]), (f, vec![a]), (d, vec![a]), (a, vec![])];
        let l = layout(commits.iter().map(|(o, p)| (*o, p.as_slice())));
        assert_eq!(l.width, 2);
        assert_eq!(l.rows[0].col, 0);
        assert_eq!(l.rows[1].col, 1);
        assert_eq!(l.rows[2].col, 0);
        assert_eq!(l.rows[3].col, 0);
        // a is waited for by both lanes: two incoming lines converge.
        let tops = l.rows[3].lines.iter().filter(|x| x.half == Half::Top).count();
        assert_eq!(tops, 2);
    }
}
