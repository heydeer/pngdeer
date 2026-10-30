//! The lossy encoders' quality search (WebP's, JPEG's): the lowest quality (0–100) whose file scores
//! at least the target, in rounds of ROUND files encoded at once.

use std::collections::BTreeMap;

/// Qualities tried per round, their files encoded at once.
const ROUND: usize = 3;
/// How far apart a round's qualities are while the target lies to one side of all tried so far.
const SPREAD: i32 = 4;

/// A file the search tried.
pub(crate) trait Tried {
    fn quality(&self) -> u32;
    fn score(&self) -> f64;
    fn size(&self) -> usize;
}

pub(crate) struct Found<T> {
    /// The lowest quality reaching the target.
    pub best: Option<T>,
    /// The best score short of it.
    pub closest: Option<T>,
    pub tries: usize,
    pub rounds: usize,
}

/// The lowest quality whose file scores at least `target`, `encode` making a round's files: around
/// `estimate` while the target lies to one side of every file so far (a `slope` points per quality
/// step guiding the aim), then across the range left (see `round`), until no quality between the
/// highest falling short and the lowest reaching it is left; `known` files count as tried. A file
/// that falls short and is already `limit` bytes ends it: higher qualities only make bigger files.
/// The qualities tried don't depend on threads, so neither does the answer.
pub(crate) fn search<T: Tried, E>(
    target: f64,
    limit: usize,
    estimate: f64,
    slope: f64,
    known: Vec<T>,
    mut encode: impl FnMut(&[u32]) -> Result<Vec<T>, E>,
) -> Result<Found<T>, E> {
    let mut tried: BTreeMap<u32, T> = known.into_iter().map(|t| (t.quality(), t)).collect();
    let (mut tries, mut rounds) = (0, 0);
    loop {
        let hi = tried.values().filter(|t| t.score() >= target).map(|t| t.quality()).min();
        let lo = tried.values().filter(|t| t.score() < target && hi.is_none_or(|h| t.quality() < h)).map(|t| t.quality()).max();
        let ends = |q: Option<u32>| q.map(|q| (q as i32, Some(tried[&q].score())));
        let (lo, hi) = (ends(lo).unwrap_or((-1, None)), ends(hi).unwrap_or((101, None)));
        if hi.0 - lo.0 <= 1 || (lo.0 >= 0 && tried[&(lo.0 as u32)].size() >= limit) {
            break;
        }
        let qualities: Vec<u32> = round(lo, hi, target, estimate, slope).into_iter().filter(|q| !tried.contains_key(q)).collect();
        for file in encode(&qualities)? {
            tried.insert(file.quality(), file);
        }
        (tries, rounds) = (tries + qualities.len(), rounds + 1);
    }
    let hi = tried.values().filter(|t| t.score() >= target).map(|t| t.quality()).min();
    let best = hi.and_then(|q| tried.remove(&q));
    let closest = tried.into_values().filter(|t| t.score() < target).max_by(|a, b| a.score().total_cmp(&b.score()).then(b.quality().cmp(&a.quality())));
    Ok(Found { best, closest, tries, rounds })
}

/// A round's qualities, strictly between `lo` (falls short) and `hi` (reaches the target), each
/// with its score (-1 and 101: none yet): all of them when ROUND or fewer are left; while one side
/// is unknown, ROUND qualities SPREAD apart around where the target should be (a `slope` step from
/// the known side, or `estimate`); otherwise spread evenly between the two.
fn round(lo: (i32, Option<f64>), hi: (i32, Option<f64>), target: f64, estimate: f64, slope: f64) -> Vec<u32> {
    let (n, span) = (ROUND as i32, (ROUND as i32 - 1) * SPREAD);
    let evenly = || (1..=n).map(|i| (lo.0 + (hi.0 - lo.0) * i / (n + 1)) as u32).collect();
    if hi.0 - lo.0 - 1 <= n {
        return (lo.0 + 1..hi.0).map(|q| q as u32).collect();
    }
    let center = match (lo.1, hi.1) {
        (Some(_), Some(_)) => return evenly(),
        _ if hi.0 - lo.0 - 2 < span => return evenly(), // too few left for the spread
        (Some(a), None) => lo.0 as f64 + (target - a) / slope,
        (None, Some(b)) => hi.0 as f64 - (b - target) / slope,
        (None, None) => estimate,
    };
    let center = (center.round() as i32).clamp(lo.0 + 1 + span / 2, hi.0 - 1 - span / 2);
    (0..n).map(|i| (center + (i - n / 2) * SPREAD) as u32).collect()
}
