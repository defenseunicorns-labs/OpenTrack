//! Track-oriented multiple hypothesis tracking.
//!
//! Each tree is one possible target; its leaves are the association
//! histories still alive. Every scan each leaf branches into "missed" and one
//! branch per plot in its gate, and every plot also starts a tree. Leaves are
//! scored by log-likelihood ratio (target against clutter). The best global
//! hypothesis, a set of compatible leaves (one per tree, each plot used
//! once) with the highest total score, is solved exactly per cluster of
//! conflicting leaves. Then scan k-N is committed: branches that disagree
//! with the global hypothesis there are pruned, as are trees that duplicate
//! a chosen older one.
//!
//! Reports carry a track number that survives the global hypothesis moving
//! from one tree to another for the same target: a newly chosen tree takes
//! over the number of a recently reported, no longer chosen track inside
//! its gate.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use super::filter::Kf;
use super::{Associate, Life, Plot, Report, TrackerSpec, secs};

/// An association: (scan number, plot index), or a miss.
type Entry = (u64, Option<usize>);

#[derive(Debug, Clone)]
struct Leaf {
    kf: Kf,
    score: f64,
    /// The last N+1 associations (older ones are committed).
    hist: Vec<Entry>,
    life: Life,
}

/// The last report under a track number.
struct Owner {
    tree: u64,
    kf: Kf,
    existence: f64,
}

// A dropped MHT track reports the stage's domain only: the tree that knew
// its plots' domains may be gone.

pub(super) struct Mht {
    spec: TrackerSpec,
    trees: BTreeMap<u64, Vec<Leaf>>,
    next_tree: u64,
    k: u64,
    /// Tree → the track number it reports under.
    labels: HashMap<u64, u64>,
    owners: HashMap<u64, Owner>,
    next_label: u64,
}

/// Trees whose best hypothesis scores below this are gone.
const DELETE_SCORE: f64 = -20.0;
/// Search budget per cluster; past it the best set found so far is used.
const SEARCH_NODES: usize = 200_000;

impl Mht {
    pub(super) fn new(spec: TrackerSpec) -> Self {
        Self {
            spec,
            trees: BTreeMap::new(),
            next_tree: 1,
            k: 0,
            labels: HashMap::new(),
            owners: HashMap::new(),
            next_label: 1,
        }
    }

    /// The best global hypothesis: tree → its chosen leaf.
    fn solve(&self) -> BTreeMap<u64, Leaf> {
        let leaves: Vec<(u64, &Leaf)> = self
            .trees
            .iter()
            .flat_map(|(t, ls)| ls.iter().map(move |l| (*t, l)))
            .filter(|(_, l)| l.score > 0.0)
            .collect();
        let n = leaves.len();
        if n == 0 {
            return BTreeMap::new();
        }
        // Leaves conflict when they share a tree or a plot.
        let mut groups: HashMap<(u8, u64, usize), Vec<usize>> = HashMap::new();
        for (i, (tree, l)) in leaves.iter().enumerate() {
            groups.entry((0, *tree, 0)).or_default().push(i);
            for (k, j) in &l.hist {
                if let Some(j) = j {
                    groups.entry((1, *k, *j)).or_default().push(i);
                }
            }
        }
        let mut adj = vec![Vec::new(); n];
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for g in groups.values().filter(|g| g.len() > 1) {
            for (a, &i) in g.iter().enumerate() {
                for &j in &g[a + 1..] {
                    adj[i].push(j);
                    adj[j].push(i);
                }
                let (ri, r0) = (find(&mut parent, i), find(&mut parent, g[0]));
                parent[ri] = r0;
            }
        }
        let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let r = find(&mut parent, i);
            clusters.entry(r).or_default().push(i);
        }
        let scores: Vec<f64> = leaves.iter().map(|(_, l)| l.score).collect();
        let mut chosen = BTreeMap::new();
        for nodes in clusters.values() {
            for i in best_independent_set(nodes, &scores, &adj) {
                chosen.insert(leaves[i].0, leaves[i].1.clone());
            }
        }
        chosen
    }

    /// The track number of a chosen tree, taking over a nearby number whose
    /// tree is no longer chosen, else a new one.
    fn label(&mut self, tree: u64, leaf: &Leaf, chosen: &BTreeMap<u64, Leaf>) -> u64 {
        if let Some(l) = self.labels.get(&tree) {
            return *l;
        }
        let q = self.spec.process_noise_mps2;
        let inherit = self
            .owners
            .iter()
            .filter(|(_, o)| !chosen.contains_key(&o.tree))
            .map(|(l, o)| {
                let then = o.kf.predict(leaf.kf.t, q);
                (
                    *l,
                    o.tree,
                    leaf.kf
                        .innovation(then.lat, then.lon, then.position_cov())
                        .d2,
                )
            })
            .filter(|(_, _, d2)| *d2 <= self.spec.gate)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let label = match inherit {
            Some((l, old, _)) => {
                self.labels.remove(&old);
                l
            }
            None => {
                self.next_label += 1;
                self.next_label - 1
            }
        };
        self.labels.insert(tree, label);
        label
    }

    /// Commit scan k-N: keep only the branches that agree with the global
    /// hypothesis there, and drop trees that lost every branch or hope.
    fn prune(&mut self, k: u64, chosen: &BTreeMap<u64, Leaf>) {
        let n = self.spec.mht.n_scan as u64;
        if k <= n {
            return;
        }
        let commit = k - n;
        let entry = |l: &Leaf| l.hist.iter().find(|h| h.0 == commit).copied();
        let mut committed: HashMap<usize, u64> = HashMap::new();
        for (tree, l) in chosen {
            if let Some((_, Some(j))) = entry(l) {
                committed.insert(j, *tree);
            }
        }
        for (tree, leaves) in self.trees.iter_mut() {
            let best = chosen.get(tree).map(entry);
            leaves.retain(|l| {
                let e = entry(l);
                if best.is_some_and(|b| b != e) {
                    return false;
                }
                !matches!(e, Some((_, Some(j))) if committed.get(&j).is_some_and(|o| o != tree))
            });
        }
        self.trees
            .retain(|_, ls| ls.iter().any(|l| l.score > DELETE_SCORE));
    }

    /// Drop unchosen trees that used a plot a chosen, older tree used: the
    /// same target twice (without this the solver alternates between twins).
    fn dedupe(&mut self, chosen: &BTreeMap<u64, Leaf>) {
        let mut owner: HashMap<Entry, u64> = HashMap::new();
        for (tree, l) in chosen {
            for e in l.hist.iter().filter(|e| e.1.is_some()) {
                let o = owner.entry(*e).or_insert(*tree);
                *o = (*o).min(*tree);
            }
        }
        self.trees.retain(|tree, ls| {
            chosen.contains_key(tree)
                || !ls.iter().any(|l| {
                    l.hist
                        .iter()
                        .any(|e| owner.get(e).is_some_and(|o| o < tree))
                })
        });
    }
}

impl Associate for Mht {
    fn spec_mut(&mut self) -> &mut TrackerSpec {
        &mut self.spec
    }

    fn scan(&mut self, t: DateTime<Utc>, plots: &[Plot]) -> Vec<Report> {
        self.k += 1;
        let k = self.k;
        let s = &self.spec;
        let m = &s.mht;
        let (pd, clutter, birth_density) = s.detection_model();
        let (ln_hit, ln_miss, ln_clutter) = (pd.ln(), (1.0 - pd).ln(), clutter.ln());
        let keep = m.n_scan + 1;
        let extend = |h: &[Entry], e: Entry| {
            let mut h = h.to_vec();
            h.push(e);
            if h.len() > keep {
                h.remove(0);
            }
            h
        };
        for leaves in self.trees.values_mut() {
            let mut out = Vec::new();
            for l in leaves.iter() {
                let pred = l.kf.predict(t, s.process_noise_mps2);
                if l.life.alive(t, s) {
                    let mut life = l.life.clone();
                    life.miss(t, s);
                    out.push(Leaf {
                        kf: pred.clone(),
                        score: l.score + ln_miss,
                        hist: extend(&l.hist, (k, None)),
                        life,
                    });
                }
                for (j, z) in plots.iter().enumerate() {
                    let inn = pred.innovation(z.lat, z.lon, z.r);
                    if inn.d2 > s.gate {
                        continue;
                    }
                    let mut life = l.life.clone();
                    life.hit(t, z, &inn, s);
                    out.push(Leaf {
                        kf: pred.update(&inn),
                        score: l.score + ln_hit + inn.ln_likelihood() - ln_clutter,
                        hist: extend(&l.hist, (k, Some(j))),
                        life,
                    });
                }
            }
            out.sort_by(|a, b| b.score.total_cmp(&a.score));
            out.truncate(m.max_branches);
            *leaves = out;
        }
        self.trees.retain(|_, ls| !ls.is_empty());
        let birth = (birth_density / clutter).ln();
        for (j, z) in plots.iter().enumerate() {
            let leaf = Leaf {
                kf: Kf::birth(z.lat, z.lon, z.r, s.initial_speed_sigma_mps, t),
                score: birth,
                hist: vec![(k, Some(j))],
                life: Life::new(t, z, s),
            };
            self.trees.insert(self.next_tree, vec![leaf]);
            self.next_tree += 1;
        }

        let chosen = self.solve();
        let mut reports = Vec::new();
        for (tree, l) in &chosen {
            if let (true, Some(&(kk, Some(j)))) = (l.life.confirmed, l.hist.last())
                && kk == k
            {
                let id = self.label(*tree, l, &chosen);
                self.owners.insert(
                    id,
                    Owner {
                        tree: *tree,
                        kf: l.kf.clone(),
                        existence: l.life.existence,
                    },
                );
                reports.push(Report {
                    id,
                    kf: l.kf.clone(),
                    domain: l.life.domain(&self.spec),
                    plot: j,
                    dropped: false,
                    existence: l.life.existence,
                });
            }
        }
        self.prune(k, &chosen);
        self.dedupe(&chosen);
        // A track number not reported for the drop time is over.
        let limit = secs(self.spec.drop_confirmed_secs);
        let spec = &self.spec;
        self.owners.retain(|id, o| {
            let alive = t - o.kf.t <= limit;
            if !alive && !plots.is_empty() {
                reports.push(Report {
                    id: *id,
                    kf: o.kf.clone(),
                    domain: spec.domain,
                    plot: 0,
                    dropped: true,
                    existence: o.existence,
                });
            }
            alive
        });
        let trees = &self.trees;
        self.labels.retain(|tree, _| trees.contains_key(tree));
        reports
    }
}

/// The highest-scoring set of leaves with no two in conflict (`adj`), among
/// `nodes`: branch and bound, best first, from a greedy start.
fn best_independent_set(nodes: &[usize], scores: &[f64], adj: &[Vec<usize>]) -> Vec<usize> {
    if nodes.len() == 1 {
        return nodes.to_vec();
    }
    let mut order = nodes.to_vec();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(i, g)| (*g, i)).collect();
    let local: Vec<Vec<usize>> = order
        .iter()
        .map(|g| adj[*g].iter().filter_map(|x| pos.get(x).copied()).collect())
        .collect();
    let w: Vec<f64> = order.iter().map(|g| scores[*g]).collect();
    let mut suffix = vec![0.0; w.len() + 1];
    for i in (0..w.len()).rev() {
        suffix[i] = suffix[i + 1] + w[i];
    }

    struct Search<'a> {
        w: &'a [f64],
        adj: &'a [Vec<usize>],
        suffix: &'a [f64],
        blocked: Vec<u32>,
        stack: Vec<usize>,
        best: f64,
        best_set: Vec<usize>,
        budget: usize,
    }
    impl Search<'_> {
        fn go(&mut self, i: usize, cur: f64) {
            if self.budget == 0 || cur + self.suffix[i] <= self.best + 1e-12 {
                return;
            }
            self.budget -= 1;
            if i == self.w.len() {
                self.best = cur;
                self.best_set = self.stack.clone();
                return;
            }
            if self.blocked[i] == 0 {
                for &a in &self.adj[i] {
                    self.blocked[a] += 1;
                }
                self.stack.push(i);
                self.go(i + 1, cur + self.w[i]);
                self.stack.pop();
                for &a in &self.adj[i] {
                    self.blocked[a] -= 1;
                }
            }
            self.go(i + 1, cur);
        }
    }

    // Greedy start: best first, skipping conflicts.
    let mut blocked = vec![false; w.len()];
    let (mut best, mut best_set) = (0.0, Vec::new());
    for i in 0..w.len() {
        if !blocked[i] {
            best += w[i];
            best_set.push(i);
            for &a in &local[i] {
                blocked[a] = true;
            }
        }
    }
    let mut s = Search {
        w: &w,
        adj: &local,
        suffix: &suffix,
        blocked: vec![0; w.len()],
        stack: Vec::new(),
        best,
        best_set,
        budget: SEARCH_NODES,
    };
    s.go(0, 0.0);
    s.best_set.into_iter().map(|i| order[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_set_beats_greedy() {
        // 0 conflicts with 1 and 2; greedy takes 0 (5), optimal takes 1 + 2 (8).
        let scores = [5.0, 4.0, 4.0];
        let adj = vec![vec![1, 2], vec![0], vec![0]];
        let mut s = best_independent_set(&[0, 1, 2], &scores, &adj);
        s.sort();
        assert_eq!(s, [1, 2]);
    }
}
