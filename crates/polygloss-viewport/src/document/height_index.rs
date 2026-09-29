//! `HeightIndex`: a Fenwick tree over item heights with O(log n) update, prefix
//! and offset lookup (design §12.4). The document keeps one over all files and
//! every laid-out file keeps one over its rows.

/// Prefix sums over item heights.
///
/// Heights are `f32` pixels, as GPUI measures them. Sums and offsets are `f64`:
/// a Linux-sized diff is tens of millions of pixels tall, where `f32` has a
/// resolution of 2 px and would make scrolling jitter. Sums of the `f32`
/// heights we store are exact in `f64` (fewer than 53 significant bits), so
/// any number of [`HeightIndex::set`] calls never drifts from the true sums.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeightIndex {
    heights: Vec<f32>,
    /// 1-based Fenwick tree: `tree[k]` is the sum of `heights[k - lowbit(k)..k]`;
    /// `tree[0]` is unused.
    tree: Vec<f64>,
    total: f64,
}

impl HeightIndex {
    /// Builds the index in O(n). Negative, NaN and infinite heights count as 0.
    pub fn new(heights: &[f32]) -> HeightIndex {
        let heights: Vec<f32> = heights.iter().map(|&h| sanitize(h)).collect();
        let n = heights.len();
        let mut tree = vec![0.0f64; n + 1];
        let mut total = 0.0;
        for (i, &h) in heights.iter().enumerate() {
            tree[i + 1] = f64::from(h);
            total += f64::from(h);
        }
        // Push each node's sum into its parent: O(n).
        for k in 1..=n {
            let parent = k + lowbit(k);
            if parent <= n {
                tree[parent] += tree[k];
            }
        }
        HeightIndex {
            heights,
            tree,
            total,
        }
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Height of item `i`. Panics if `i` is out of range.
    pub fn get(&self, i: usize) -> f32 {
        self.heights[i]
    }

    /// Sets the height of item `i` in O(log n). Negative, NaN and infinite
    /// heights count as 0. Panics if `i` is out of range.
    pub fn set(&mut self, i: usize, h: f32) {
        let h = sanitize(h);
        let delta = f64::from(h) - f64::from(self.heights[i]);
        if delta == 0.0 {
            return;
        }
        self.heights[i] = h;
        self.total += delta;
        let n = self.heights.len();
        let mut k = i + 1;
        while k <= n {
            self.tree[k] += delta;
            k += lowbit(k);
        }
    }

    /// Sum of the heights of items `0..i` (the top of item `i`); `i` past the
    /// end is clamped, so `prefix(len())` is the total.
    pub fn prefix(&self, i: usize) -> f64 {
        let mut k = i.min(self.heights.len());
        let mut sum = 0.0;
        while k > 0 {
            sum += self.tree[k];
            k -= lowbit(k);
        }
        sum
    }

    /// Sum of all heights, O(1).
    pub fn total(&self) -> f64 {
        self.total
    }

    /// The item containing `offset` and the offset within it: the first item
    /// `i` with `prefix(i + 1) > offset`, so zero-height items are skipped and
    /// an offset on a boundary belongs to the item that starts there. Offsets
    /// below 0 clamp to 0; offsets at or past the total return the last item
    /// (with an offset that may exceed its height). An empty index returns
    /// `(0, 0.0)`.
    pub fn find(&self, offset: f64) -> (usize, f64) {
        let n = self.heights.len();
        if n == 0 {
            return (0, 0.0);
        }
        // `offset > 0.0` is false for NaN too.
        let offset = if offset > 0.0 { offset } else { 0.0 };
        // Binary lifting: the largest `pos` with `prefix(pos) <= offset`, i.e.
        // the number of items that end at or before `offset`.
        let mut pos = 0;
        let mut rest = offset;
        let mut step = 1usize << (usize::BITS - 1 - n.leading_zeros());
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= rest {
                pos = next;
                rest -= self.tree[next];
            }
            step >>= 1;
        }
        if pos < n {
            (pos, rest)
        } else {
            (n - 1, offset - self.prefix(n - 1))
        }
    }

    /// Bytes owned on the heap.
    pub fn heap_bytes(&self) -> usize {
        self.heights.capacity() * size_of::<f32>() + self.tree.capacity() * size_of::<f64>()
    }
}

/// Heights are finite and non-negative; anything else counts as 0.
fn sanitize(h: f32) -> f32 {
    if h.is_finite() && h > 0.0 { h } else { 0.0 }
}

/// The lowest set bit of `k` (Fenwick step).
fn lowbit(k: usize) -> usize {
    k.isolate_lowest_one()
}
