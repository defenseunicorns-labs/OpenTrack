//! Minimum-cost assignment (the Hungarian method, O(n²m)).

/// For a cost matrix (rows × columns), the column assigned to each row with
/// the least total cost, every row and column used at most once. Rows beyond
/// the number of columns stay unassigned (None).
pub fn assign(cost: &[Vec<f64>]) -> Vec<Option<usize>> {
    let n = cost.len();
    let m = cost.first().map_or(0, Vec::len);
    if n == 0 || m == 0 {
        return vec![None; n];
    }
    if n > m {
        // Solve the transpose: each column picks a row.
        let t: Vec<Vec<f64>> = (0..m)
            .map(|j| (0..n).map(|i| cost[i][j]).collect())
            .collect();
        let mut out = vec![None; n];
        for (j, i) in assign(&t).into_iter().enumerate() {
            if let Some(i) = i {
                out[i] = Some(j);
            }
        }
        return out;
    }
    // Potentials u (rows), v (columns); p[j] = row matched to column j (1-based, 0 = none).
    let inf = f64::INFINITY;
    let (mut u, mut v) = (vec![0.0; n + 1], vec![0.0; m + 1]);
    let (mut p, mut way) = (vec![0usize; m + 1], vec![0usize; m + 1]);
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![None; n];
    for j in 1..=m {
        if p[j] != 0 {
            out[p[j] - 1] = Some(j - 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(cost: &[Vec<f64>], a: &[Option<usize>]) -> f64 {
        a.iter()
            .enumerate()
            .filter_map(|(i, j)| j.map(|j| cost[i][j]))
            .sum()
    }

    #[test]
    fn beats_greedy_on_a_contested_pair() {
        // Greedy takes (0,0)=1 then (1,1)=10: 11. Optimal: (0,1)+(1,0) = 2+2.
        let cost = vec![vec![1.0, 2.0], vec![2.0, 10.0]];
        let a = assign(&cost);
        assert_eq!(a, vec![Some(1), Some(0)]);
        assert_eq!(total(&cost, &a), 4.0);
    }

    #[test]
    fn handles_rectangles_both_ways() {
        let wide = vec![vec![5.0, 1.0, 9.0]];
        assert_eq!(assign(&wide), vec![Some(1)]);
        let tall = vec![vec![5.0], vec![1.0], vec![9.0]];
        assert_eq!(assign(&tall), vec![None, Some(0), None]);
        assert!(assign(&[]).is_empty());
    }

    #[test]
    fn matches_brute_force() {
        // A fixed pseudo-random 5×6 matrix against every permutation.
        let mut x = 7u64;
        let mut rnd = || {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as f64 / (1u64 << 31) as f64
        };
        let cost: Vec<Vec<f64>> = (0..5).map(|_| (0..6).map(|_| rnd()).collect()).collect();
        fn best(cost: &[Vec<f64>], i: usize, used: &mut Vec<bool>) -> f64 {
            if i == cost.len() {
                return 0.0;
            }
            let mut b = f64::INFINITY;
            for j in 0..cost[0].len() {
                if !used[j] {
                    used[j] = true;
                    b = b.min(cost[i][j] + best(cost, i + 1, used));
                    used[j] = false;
                }
            }
            b
        }
        let a = assign(&cost);
        assert!((total(&cost, &a) - best(&cost, 0, &mut vec![false; 6])).abs() < 1e-9);
    }
}
