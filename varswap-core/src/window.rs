//! The window account: compact bucket storage, sampling and finalize (`docs/PLAN.md` 3.1,
//! 3.3, 4.1, 4.3).

use core::mem::size_of;

use pyth_lite::UnixTimestamp;

use crate::error::CoreError;
use crate::kernel::{ReturnKernel, SumSq};
use crate::layout::{Hdr, WindowHeader};
use crate::print::{Print, StoredPrice};
use crate::spec::{BucketRun, WindowSpec};
use crate::types::WindowState;

/// Account kind tag of a window.
pub const WINDOW_TAG: u8 = 1;
/// Layout version of a window.
pub const WINDOW_VERSION: u8 = 1;
/// Largest account a program can create through a CPI, bytes
/// (`MAX_PERMITTED_DATA_INCREASE`; checked with LiteSVM in milestone 3).
pub const CPI_ACCOUNT_CREATION_LIMIT: usize = 10_240;
/// Largest number of buckets `B = N + 1` whose window fits in
/// [`CPI_ACCOUNT_CREATION_LIMIT`]: `window_size(1_105) = 10_233`, `window_size(1_106) =
/// 10_242`.
pub const MAX_BUCKETS: u32 = 1_105;

const HEADER_LEN: usize = size_of::<WindowHeader>();
const WORD_BITS: u32 = 64;

/// Serialized size of a window with `buckets` buckets: `size_of::<WindowHeader>() + 8 ×
/// ceil(B / 64) + 9 × B` bytes.
///
/// # Errors
///
/// [`CoreError::Overflow`].
pub fn window_size(buckets: u32) -> Result<usize, CoreError> {
    let b = usize::try_from(buckets).map_err(|_| CoreError::Overflow)?;
    bitmap_words(buckets)?
        .checked_mul(8)
        .and_then(|bitmap| {
            b.checked_mul(9)
                .and_then(|arrays| arrays.checked_add(bitmap))
        })
        .and_then(|body| body.checked_add(HEADER_LEN))
        .ok_or(CoreError::Overflow)
}

/// `ceil(B / 64)`.
fn bitmap_words(buckets: u32) -> Result<usize, CoreError> {
    let words = buckets
        .checked_add(WORD_BITS - 1)
        .and_then(|b| b.checked_div(WORD_BITS))
        .ok_or(CoreError::Overflow)?;
    usize::try_from(words).map_err(|_| CoreError::Overflow)
}

/// A window account opened over its bytes. Reads copy the header out (no alignment
/// requirement on `data`); every mutation validates first and writes last, so a rejected
/// call leaves the bytes untouched.
pub struct Window<'a> {
    data: &'a mut [u8],
    header: WindowHeader,
    spec: WindowSpec,
    buckets: u32,
}

impl<'a> Window<'a> {
    /// Initializes an account of exactly [`window_size`] bytes whose header and bitmap are
    /// zero (a freshly created account): state `Pending`, every bucket empty.
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] if the size is wrong or the header or bitmap is not
    /// zero (re-initialization).
    pub fn init(data: &'a mut [u8], spec: WindowSpec, bump: u8) -> Result<Self, CoreError> {
        let buckets = spec.buckets()?;
        if data.len() != window_size(buckets)? {
            return Err(CoreError::InvalidAccountData);
        }
        // Header and bitmap must be zero: an initialized window has a non-zero tag, and the
        // bitmap alone decides which buckets are filled (prices are read only behind a set
        // bit). Scanning only these keeps `create_window` linear in the bitmap words, not
        // in the account size.
        let bitmap_end = bitmap_words(buckets)?
            .checked_mul(8)
            .and_then(|b| b.checked_add(HEADER_LEN))
            .ok_or(CoreError::Overflow)?;
        if data
            .get(..bitmap_end)
            .ok_or(CoreError::InvalidAccountData)?
            .iter()
            .any(|b| *b != 0)
        {
            return Err(CoreError::InvalidAccountData);
        }
        let header = WindowHeader {
            hdr: Hdr {
                tag: WINDOW_TAG,
                version: WINDOW_VERSION,
                bump,
                _pad: [0; 5],
            },
            spec: spec.to_raw(),
            state: WindowState::Pending as u8,
            _pad: [0; 3],
            filled: 0,
            sum_sq: [0; 16],
            realized_var: [0; 16],
        };
        let mut w = Self {
            data,
            header,
            spec,
            buckets,
        };
        w.store_header()?;
        Ok(w)
    }

    /// Opens an initialized window and validates it: tag, version, spec (as
    /// [`WindowSpec::try_new`]), exact size, state, padding, `filled` equal to the number of
    /// bits set (none past `B`), and the state consistent with `filled`. Cost is linear in the
    /// bitmap words (at most 18), not in the buckets.
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] on any mismatch.
    pub fn load(data: &'a mut [u8]) -> Result<Self, CoreError> {
        let head = data
            .get(..HEADER_LEN)
            .ok_or(CoreError::InvalidAccountData)?;
        let header: WindowHeader = bytemuck::pod_read_unaligned(head);
        if header.hdr.tag != WINDOW_TAG
            || header.hdr.version != WINDOW_VERSION
            || header.hdr._pad != [0; 5]
            || header._pad != [0; 3]
        {
            return Err(CoreError::InvalidAccountData);
        }
        let spec = WindowSpec::from_raw(&header.spec).map_err(|_| CoreError::InvalidAccountData)?;
        let buckets = spec.buckets()?;
        if data.len() != window_size(buckets)? {
            return Err(CoreError::InvalidAccountData);
        }
        let state = WindowState::try_from(header.state)?;
        let w = Self {
            data,
            header,
            spec,
            buckets,
        };
        // O(words), not O(buckets): load runs on every instruction. A filled bucket's price
        // is validated when it is read ([`Window::price`]).
        let words = bitmap_words(buckets)?;
        let mut set = 0u32;
        for i in 0..words {
            set = set
                .checked_add(w.word(i)?.count_ones())
                .ok_or(CoreError::Overflow)?;
        }
        // Bits past B in the last bitmap word must be clear.
        let tail = buckets.checked_rem(WORD_BITS).ok_or(CoreError::Overflow)?;
        if tail != 0 {
            let last = w.word(words.checked_sub(1).ok_or(CoreError::Overflow)?)?;
            if last.checked_shr(tail).unwrap_or(0) != 0 {
                return Err(CoreError::InvalidAccountData);
            }
        }
        let consistent = match state {
            WindowState::Pending => set == 0,
            WindowState::Sampling => set > 0,
            WindowState::Finalized => set == buckets,
            WindowState::Degraded => set < buckets,
        };
        if set != header.filled || !consistent {
            return Err(CoreError::InvalidAccountData);
        }
        Ok(w)
    }

    /// The window's spec.
    #[inline]
    #[must_use]
    pub const fn spec(&self) -> &WindowSpec {
        &self.spec
    }

    /// PDA bump stored at creation.
    #[inline]
    #[must_use]
    pub const fn bump(&self) -> u8 {
        self.header.hdr.bump
    }

    /// Current state.
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] (validated at load).
    #[inline]
    pub fn state(&self) -> Result<WindowState, CoreError> {
        WindowState::try_from(self.header.state)
    }

    /// Number of filled buckets.
    #[inline]
    #[must_use]
    pub const fn filled(&self) -> u32 {
        self.header.filled
    }

    /// Number of empty buckets; any at the deadline degrades the window (4.3).
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (unreachable: `filled <= B`).
    #[inline]
    pub fn missing(&self) -> Result<u32, CoreError> {
        self.buckets
            .checked_sub(self.header.filled)
            .ok_or(CoreError::Overflow)
    }

    /// Running sum of squared log returns over adjacent filled pairs.
    #[inline]
    #[must_use]
    pub fn sum_sq(&self) -> SumSq {
        SumSq::from_raw(u128::from_le_bytes(self.header.sum_sq))
    }

    /// Annualized realized variance, set only when `Finalized`.
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] (validated at load).
    #[inline]
    pub fn realized_var(&self) -> Result<Option<u128>, CoreError> {
        Ok(match self.state()? {
            WindowState::Finalized => Some(u128::from_le_bytes(self.header.realized_var)),
            _ => None,
        })
    }

    /// Whether bucket `k` is filled.
    ///
    /// # Errors
    ///
    /// [`CoreError::NotOnGrid`] if `k > N`.
    pub fn is_filled(&self, k: u32) -> Result<bool, CoreError> {
        let (word, mask) = self.bit(k)?;
        Ok(self.word(word)? & mask != 0)
    }

    /// The price stored in bucket `k`, or `None` if it is empty.
    ///
    /// # Errors
    ///
    /// [`CoreError::NotOnGrid`] if `k > N`, [`CoreError::InvalidAccountData`] if a filled
    /// bucket holds a non-positive price.
    pub fn price(&self, k: u32) -> Result<Option<StoredPrice>, CoreError> {
        if !self.is_filled(k)? {
            return Ok(None);
        }
        self.stored_price(k).map(Some)
    }

    /// Records a canonical print: every bucket of its run takes its price, and the squared
    /// log returns to the already filled neighbours `first − 1` and `last + 1` are added to
    /// `sum_sq` (returns inside the run are zero). At most two kernel calls.
    ///
    /// The caller verified the price update with `pyth-lite` (owner =
    /// `spec.receiver_program()`, `Full`, feed, positive price, `max_conf`) and passes the
    /// `Clock` time. Returns the run filled.
    ///
    /// # Errors
    ///
    /// [`CoreError::WindowFinal`], [`CoreError::BackfillClosed`] if `now > end +
    /// backfill_deadline`, [`CoreError::WrongFeed`], [`CoreError::NotOnGrid`],
    /// [`CoreError::BucketAlreadySampled`], or a kernel error. Nothing is written on error.
    pub fn sample<K: ReturnKernel>(
        &mut self,
        print: &Print,
        now: UnixTimestamp,
        kernel: &K,
    ) -> Result<BucketRun, CoreError> {
        if self.state()?.is_final() {
            return Err(CoreError::WindowFinal);
        }
        if now > self.spec.deadline()? {
            return Err(CoreError::BackfillClosed);
        }
        if print.feed_id() != self.spec.feed() {
            return Err(CoreError::WrongFeed);
        }
        let run = self.spec.canonical_run(print)?;
        for k in run.first()..=run.last() {
            if self.is_filled(k)? {
                return Err(CoreError::BucketAlreadySampled);
            }
        }
        let new = print.stored();
        let mut sum = self.sum_sq();
        if let Some(before) = run.first().checked_sub(1) {
            if let Some(p) = self.price(before)? {
                sum = sum.checked_add(kernel.squared_log_return(p, new)?)?;
            }
        }
        if run.last() < self.spec.last_index() {
            let after = run.last().checked_add(1).ok_or(CoreError::Overflow)?;
            if let Some(p) = self.price(after)? {
                sum = sum.checked_add(kernel.squared_log_return(new, p)?)?;
            }
        }
        let filled = self
            .header
            .filled
            .checked_add(run.count()?)
            .ok_or(CoreError::Overflow)?;

        for k in run.first()..=run.last() {
            self.write_bucket(k, new)?;
        }
        self.header.filled = filled;
        self.header.sum_sq = sum.raw().to_le_bytes();
        self.header.state = WindowState::Sampling as u8;
        self.store_header()?;
        Ok(run)
    }

    /// Ends the window. With every bucket filled it is `Finalized` at once and
    /// `realized_var = annualize(sum_sq, end − start)`; otherwise, only once `now > end +
    /// backfill_deadline`, it is `Degraded` and its swaps unwind (4.3). Nobody can degrade a
    /// window early.
    ///
    /// # Errors
    ///
    /// [`CoreError::WindowFinal`], [`CoreError::FinalizeTooEarly`], or a kernel error.
    pub fn finalize<K: ReturnKernel>(
        &mut self,
        now: UnixTimestamp,
        kernel: &K,
    ) -> Result<WindowState, CoreError> {
        if self.state()?.is_final() {
            return Err(CoreError::WindowFinal);
        }
        let state = if self.header.filled == self.buckets {
            let var = kernel.annualize(self.sum_sq(), self.spec.span_seconds()?)?;
            self.header.realized_var = var.to_le_bytes();
            WindowState::Finalized
        } else if now > self.spec.deadline()? {
            WindowState::Degraded
        } else {
            return Err(CoreError::FinalizeTooEarly);
        };
        self.header.state = state as u8;
        self.store_header()?;
        Ok(state)
    }

    fn store_header(&mut self) -> Result<(), CoreError> {
        self.data
            .get_mut(..HEADER_LEN)
            .ok_or(CoreError::InvalidAccountData)?
            .copy_from_slice(bytemuck::bytes_of(&self.header));
        Ok(())
    }

    /// Bitmap word index and mask of bucket `k`.
    fn bit(&self, k: u32) -> Result<(usize, u64), CoreError> {
        if k >= self.buckets {
            return Err(CoreError::NotOnGrid);
        }
        let word = k.checked_div(WORD_BITS).ok_or(CoreError::Overflow)?;
        let bit = k.checked_rem(WORD_BITS).ok_or(CoreError::Overflow)?;
        let mask = 1u64.checked_shl(bit).ok_or(CoreError::Overflow)?;
        Ok((
            usize::try_from(word).map_err(|_| CoreError::Overflow)?,
            mask,
        ))
    }

    fn word(&self, i: usize) -> Result<u64, CoreError> {
        let off = i
            .checked_mul(8)
            .and_then(|o| o.checked_add(HEADER_LEN))
            .ok_or(CoreError::Overflow)?;
        self.read::<8>(off).map(u64::from_le_bytes)
    }

    fn prices_offset(&self) -> Result<usize, CoreError> {
        bitmap_words(self.buckets)?
            .checked_mul(8)
            .and_then(|b| b.checked_add(HEADER_LEN))
            .ok_or(CoreError::Overflow)
    }

    fn price_offset(&self, k: u32) -> Result<usize, CoreError> {
        let k = usize::try_from(k).map_err(|_| CoreError::Overflow)?;
        k.checked_mul(8)
            .and_then(|o| o.checked_add(self.prices_offset().ok()?))
            .ok_or(CoreError::Overflow)
    }

    fn exponent_offset(&self, k: u32) -> Result<usize, CoreError> {
        let b = usize::try_from(self.buckets).map_err(|_| CoreError::Overflow)?;
        let k = usize::try_from(k).map_err(|_| CoreError::Overflow)?;
        b.checked_mul(8)
            .and_then(|o| o.checked_add(self.prices_offset().ok()?))
            .and_then(|o| o.checked_add(k))
            .ok_or(CoreError::Overflow)
    }

    fn stored_price(&self, k: u32) -> Result<StoredPrice, CoreError> {
        let price = i64::from_le_bytes(self.read::<8>(self.price_offset(k)?)?);
        let [e] = self.read::<1>(self.exponent_offset(k)?)?;
        StoredPrice::try_new(price, i8::from_le_bytes([e]))
    }

    fn write_bucket(&mut self, k: u32, p: StoredPrice) -> Result<(), CoreError> {
        let (word, mask) = self.bit(k)?;
        let bits = self.word(word)? | mask;
        let word_off = word
            .checked_mul(8)
            .and_then(|o| o.checked_add(HEADER_LEN))
            .ok_or(CoreError::Overflow)?;
        let price_off = self.price_offset(k)?;
        let exp_off = self.exponent_offset(k)?;
        self.write(word_off, &bits.to_le_bytes())?;
        self.write(price_off, &p.price().to_le_bytes())?;
        self.write(exp_off, &p.exponent().to_le_bytes())
    }

    fn read<const N: usize>(&self, off: usize) -> Result<[u8; N], CoreError> {
        self.data
            .get(off..)
            .and_then(|s| s.first_chunk::<N>())
            .copied()
            .ok_or(CoreError::InvalidAccountData)
    }

    fn write(&mut self, off: usize, bytes: &[u8]) -> Result<(), CoreError> {
        let end = off.checked_add(bytes.len()).ok_or(CoreError::Overflow)?;
        self.data
            .get_mut(off..end)
            .ok_or(CoreError::InvalidAccountData)?
            .copy_from_slice(bytes);
        Ok(())
    }
}
