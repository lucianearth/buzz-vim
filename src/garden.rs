//! The loading garden: five flowers grow out of the ground one after another,
//! then sway while a bee visits them. Everything is laid out on a cell grid,
//! so stems, leaves and heads always line up, and every frame is the same
//! size so nothing around it moves.

/// What a cell is, so the caller can pick a style per part.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Empty,
    Ground,
    Stem,
    Leaf,
    Bud,
    Bloom,
    Bee,
}

pub const FLOWERS: usize = 5;
/// Columns between stems.
const GAP: usize = 5;
pub const WIDTH: usize = GAP * FLOWERS + 1;
/// Ground row plus the tallest flower plus one row for the bee.
pub const HEIGHT: usize = 8;

/// Ticks between one flower starting and the next, and per growth step.
const START_EVERY: u64 = 10;
const STEP: u64 = 2;

struct Flower {
    /// Stem rows above the ground.
    stem: usize,
    /// Stem rows (1-based from the ground) that get a pair of leaves.
    leaves: &'static [usize],
    heads: [char; 2],
}

const BED: [Flower; FLOWERS] = [
    Flower {
        stem: 3,
        leaves: &[1],
        heads: ['✿', '❀'],
    },
    Flower {
        stem: 5,
        leaves: &[2],
        heads: ['❀', '✿'],
    },
    Flower {
        stem: 2,
        leaves: &[1],
        heads: ['✾', '✿'],
    },
    Flower {
        stem: 4,
        leaves: &[1, 3],
        heads: ['✿', '✾'],
    },
    Flower {
        stem: 3,
        leaves: &[2],
        heads: ['❀', '✾'],
    },
];

pub type Grid = Vec<Vec<(char, Part)>>;

/// The garden `ticks` after it started. `grow: false` shows it fully grown
/// and still (for empty screens), with no bee.
pub fn frame(ticks: u64, grow: bool) -> Grid {
    let mut g = vec![vec![(' ', Part::Empty); WIDTH]; HEIGHT];
    let ground = HEIGHT - 1;
    for cell in &mut g[ground] {
        *cell = ('▔', Part::Ground);
    }
    let mut all_bloomed = true;
    for (i, fl) in BED.iter().enumerate() {
        let x = GAP / 2 + 1 + i * GAP;
        // Steps: 0 sprout, 1..=stem grow the stem, then bud, then bloom.
        let step = if grow {
            let since = ticks.checked_sub(i as u64 * START_EVERY);
            match since {
                None => {
                    all_bloomed = false;
                    continue;
                }
                Some(t) => (t / STEP) as usize,
            }
        } else {
            usize::MAX
        };
        let bloom_at = fl.stem + 2;
        if step < bloom_at {
            all_bloomed = false;
        }
        if step == 0 {
            g[ground - 1][x] = (',', Part::Stem);
            continue;
        }
        let rows = step.min(fl.stem);
        for r in 1..=rows {
            g[ground - r][x] = ('│', Part::Stem);
            if fl.leaves.contains(&r) && step > r {
                g[ground - r][x - 1] = ('╲', Part::Leaf);
                g[ground - r][x + 1] = ('╱', Part::Leaf);
            }
        }
        let top = ground - fl.stem - 1;
        if step == fl.stem + 1 {
            g[top][x] = ('•', Part::Bud);
        } else if step >= bloom_at {
            // Heads sway once the whole bed is up; each on its own beat.
            let sway = grow && ((ticks / 4) + i as u64) % 2 == 1;
            g[top][x] = (fl.heads[sway as usize], Part::Bloom);
        }
    }
    if grow && all_bloomed {
        // The bee hops along the heads, hovering just above each one.
        let i = ((ticks / 6) as usize) % FLOWERS;
        let x = GAP / 2 + 1 + i * GAP;
        let y = HEIGHT - 1 - BED[i].stem - 2;
        let wing = if ticks.is_multiple_of(2) { 'ʚ' } else { 'ɞ' };
        g[y][x - 1] = (wing, Part::Bee);
        g[y][x] = ('●', Part::Bee);
    }
    g
}

/// Ticks until the last flower blooms.
#[cfg(test)]
fn full_at() -> u64 {
    let last = &BED[FLOWERS - 1];
    (FLOWERS as u64 - 1) * START_EVERY + (last.stem as u64 + 2) * STEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthChar;

    fn text(g: &Grid) -> String {
        g.iter()
            .map(|r| r.iter().map(|c| c.0).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_frame_has_the_same_shape() {
        for t in 0..200 {
            for grow in [true, false] {
                let g = frame(t, grow);
                assert_eq!(g.len(), HEIGHT);
                for row in &g {
                    assert_eq!(row.len(), WIDTH);
                    assert!(row.iter().all(|c| c.0.width() == Some(1)), "{}", text(&g));
                }
            }
        }
    }

    #[test]
    fn heads_and_leaves_sit_on_their_stem() {
        let g = frame(0, false);
        for (y, row) in g.iter().enumerate() {
            for (x, &(_, p)) in row.iter().enumerate() {
                match p {
                    Part::Bloom => assert_eq!(g[y + 1][x].1, Part::Stem),
                    Part::Leaf => {
                        let stem = if x > 0 && g[y][x - 1].1 == Part::Stem {
                            x - 1
                        } else {
                            x + 1
                        };
                        assert_eq!(g[y][stem].1, Part::Stem, "{}", text(&g));
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn grows_to_five_blooms_then_the_bee_comes() {
        let blooms = |g: &Grid| g.iter().flatten().filter(|c| c.1 == Part::Bloom).count();
        let bee = |g: &Grid| g.iter().flatten().any(|c| c.1 == Part::Bee);
        assert_eq!(blooms(&frame(0, true)), 0);
        assert!(!bee(&frame(full_at() - 1, true)));
        assert_eq!(blooms(&frame(full_at(), true)), FLOWERS);
        assert!(bee(&frame(full_at(), true)));
        assert_eq!(blooms(&frame(0, false)), FLOWERS);
        assert!(!bee(&frame(0, false)));
    }
}
