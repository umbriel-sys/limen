//! Small shared types and identifiers used throughout the core.

// ***** Tracing *****

/// A 64-bit trace identifier used to correlate messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId(u64);

impl TraceId {
    /// creates a new [`TraceId`].
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the inner u64.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// A 64-bit sequence number assigned by sources or routers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SequenceNumber(u64);

impl SequenceNumber {
    /// creates a new [`SequenceNumber`].
    pub const fn new(sequence_number: u64) -> Self {
        Self(sequence_number)
    }

    /// Return the inner u64.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

// ***** Timing *****

/// Monotonic tick unit from the platform clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(u64);

impl Ticks {
    /// creates a new [`Ticks`].
    pub const fn new(ticks: u64) -> Self {
        Self(ticks)
    }

    /// Wrapping (modular) subtraction. Computes `self - rhs`,
    /// wrapping around at the boundary of the type.
    #[inline]
    pub const fn wrapping_sub(self, rhs: Ticks) -> Ticks {
        Ticks(self.0.wrapping_sub(rhs.0))
    }

    /// Wrapping (modular) addition. Computes `self + rhs`,
    /// wrapping around at the boundary of the type.
    #[inline]
    pub const fn wrapping_add(self, rhs: Ticks) -> Ticks {
        Ticks(self.0.wrapping_add(rhs.0))
    }

    /// Saturating addition. Computes `self + rhs`, saturating at `u64::MAX`.
    #[inline]
    pub const fn saturating_add(self, rhs: Ticks) -> Ticks {
        Ticks(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction. Computes `self - rhs`, saturating at `0`.
    #[inline]
    pub const fn saturating_sub(self, rhs: Ticks) -> Ticks {
        Ticks(self.0.saturating_sub(rhs.0))
    }

    /// Return the inner u64.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Absolute deadline in nanoseconds since platform boot (or epoch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeadlineNs(u64);

impl DeadlineNs {
    /// creates a new [`DeadlineNs`].
    pub const fn new(ns: u64) -> Self {
        Self(ns)
    }

    /// Return the inner u64.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

// ***** Policy *****

/// Quality-of-Service class label attached to messages and used by admission.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QoSClass {
    /// Latency-critical traffic; favored by EDF schedulers.
    LatencyCritical,
    /// Default best-effort traffic.
    BestEffort,
    /// Background/low-priority traffic.
    Background,
}

// ***** Routing *****

/// Port ID (node and port index) for an inout or output port on a node.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId {
    /// A logical index of a node in a graph.
    node: NodeIndex,
    /// A logical index for input or output ports on a node.
    port: PortIndex,
}

impl PortId {
    /// Create a new `PortId`.
    #[inline]
    pub const fn new(node: NodeIndex, port: PortIndex) -> Self {
        Self { node, port }
    }

    /// Return the node index.
    pub fn node(&self) -> NodeIndex {
        self.node
    }

    /// Return the port index.
    pub fn port(&self) -> PortIndex {
        self.port
    }
}

/// A logical index for input or output ports on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortIndex(usize);

impl core::convert::From<usize> for PortIndex {
    #[inline]
    fn from(value: usize) -> Self {
        PortIndex(value)
    }
}

impl PortIndex {
    /// Create a new `PortIndex`.
    #[inline]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Return the inner usize held.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// A logical index of a node in a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeIndex(usize);

impl core::convert::From<usize> for NodeIndex {
    #[inline]
    fn from(value: usize) -> Self {
        NodeIndex(value)
    }
}

impl NodeIndex {
    /// Create a new `NodeIndex`.
    #[inline]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Return the inner usize held.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// A logical index of an edge in a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeIndex(usize);

impl core::convert::From<usize> for EdgeIndex {
    #[inline]
    fn from(value: usize) -> Self {
        EdgeIndex(value)
    }
}

impl EdgeIndex {
    /// Create a new `EdgeIndex`.
    #[inline]
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Return the inner usize held.
    #[inline]
    pub fn as_usize(self) -> usize {
        self.0
    }
}

// ***** Payload *****

/// Supported primitive data types for tensors and values.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// Boolean value (1 byte).
    Boolean,
    /// Unsigned 8-bit integer.
    Unsigned8,
    /// Unsigned 16-bit integer.
    Unsigned16,
    /// Unsigned 32-bit integer.
    Unsigned32,
    /// Unsigned 64-bit integer.
    Unsigned64,
    /// Signed 8-bit integer.
    Signed8,
    /// Signed 16-bit integer.
    Signed16,
    /// Signed 32-bit integer.
    Signed32,
    /// Signed 64-bit integer.
    Signed64,
    /// 16-bit IEEE 754 floating point.
    Float16,
    /// 16-bit Brain floating point format.
    BFloat16,
    /// 32-bit IEEE 754 floating point.
    Float32,
    /// 64-bit IEEE 754 floating point.
    Float64,
}

impl DataType {
    /// Return the size in bytes of a single value of this type.
    pub fn byte_size(&self) -> usize {
        match self {
            DataType::Boolean => 1,
            DataType::Unsigned8 => 1,
            DataType::Signed8 => 1,
            DataType::Unsigned16 => 2,
            DataType::Signed16 => 2,
            DataType::Unsigned32 => 4,
            DataType::Signed32 => 4,
            DataType::Unsigned64 => 8,
            DataType::Signed64 => 8,
            DataType::Float16 => 2,
            DataType::BFloat16 => 2,
            DataType::Float32 => 4,
            DataType::Float64 => 8,
        }
    }
}

/// Marker trait mapping a Rust scalar to `DataType`.
pub trait DType {
    /// Primitive data type inside a Payload.
    const DATA_TYPE: DataType;
}

// ===== 16-bit Floating-Point Types (no_std, no_alloc) =======================
//
// These types are ABI-compatible with `u16` (`#[repr(transparent)]`) and do not
// allocate. Conversions use IEEE-754 round-to-nearest, ties-to-even.
//
// - `F16`   : IEEE-754 binary16 ("float16")  => 1 sign | 5 exponent | 10 fraction
//             exponent bias = 15
// - `BF16`  : bfloat16 ("Brain float 16")    => 1 sign | 8 exponent | 7 fraction
//             exponent bias = 127   (same dynamic range as f32)
//
// Both types provide:
//   * `from_bits`, `to_bits`  (bit-preserving constructors/accessors)
//   * `from_f32`, `to_f32`    (lossy conversions with RNTE rounding)
//   * Classification helpers: `is_nan`, `is_infinite`, `is_finite`,
//     `is_normal`, `is_subnormal`, `is_sign_negative`, `is_sign_positive`
//
// Note: Arithmetic is intentionally not implemented here to keep the surface
// area minimal and portable. Callers can widen to `f32` for math and
// re-quantize back to `F16` / `BF16` as needed.

/// IEEE-754 binary16 ("float16") stored as raw 16-bit bits.
///
/// Layout: 1 sign bit, 5 exponent bits (bias 15), 10 fraction bits.
///
/// This type is `#[repr(transparent)]` over `u16` and therefore has the exact
/// same size and alignment as `u16`. No heap allocations are performed.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct F16(u16);

impl core::fmt::Debug for F16 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Debug prints both the numeric value (via f32) and the raw bits.
        write!(
            f,
            "F16 {{ value: {}, bits: 0x{:04X} }}",
            self.to_f32(),
            self.0
        )
    }
}

impl F16 {
    /// Construct from raw bits (bit pattern is preserved).
    #[inline]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// Return the raw 16-bit representation.
    #[inline]
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    /// True if the number is NaN.
    #[inline]
    pub const fn is_nan(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        let frac = self.0 & 0x03FF;
        exp == 0x1F && frac != 0
    }

    /// True if the number is ±infinity.
    #[inline]
    pub const fn is_infinite(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        let frac = self.0 & 0x03FF;
        exp == 0x1F && frac == 0
    }

    /// True if the number is finite (not NaN or ±inf).
    #[inline]
    pub const fn is_finite(self) -> bool {
        ((self.0 >> 10) & 0x1F) != 0x1F
    }

    /// True if the number is a normal (non-subnormal, non-zero) finite value.
    #[inline]
    pub const fn is_normal(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        exp != 0 && exp != 0x1F
    }

    /// True if the number is subnormal (denormal).
    #[inline]
    pub const fn is_subnormal(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        let frac = self.0 & 0x03FF;
        exp == 0 && frac != 0
    }

    /// True if the sign bit is set (negative, including -0.0).
    #[inline]
    pub const fn is_sign_negative(self) -> bool {
        (self.0 & 0x8000) != 0
    }

    /// True if the sign bit is not set (positive, including +0.0).
    #[inline]
    pub const fn is_sign_positive(self) -> bool {
        (self.0 & 0x8000) == 0
    }

    /// Convert to `f32` by widening (exact for all representable values).
    #[inline]
    pub fn to_f32(self) -> f32 {
        let bits = self.0 as u32;

        let sign = (bits & 0x8000) << 16;
        let exp = (bits >> 10) & 0x1F;
        let frac = bits & 0x03FF;

        let f32_bits = if exp == 0x1F {
            // Inf or NaN
            let mant32 = frac << 13;
            if mant32 == 0 {
                // ±Inf
                sign | 0x7F80_0000
            } else {
                // NaN: set exponent to all 1s and preserve payload.
                sign | 0x7F80_0000 | mant32
            }
        } else if exp == 0 {
            if frac == 0 {
                // ±0.0
                sign
            } else {
                // Subnormal: normalize the fraction then adjust exponent.
                let mut frac_norm = frac;
                let mut exp32: i32 = 113; // 127 - 14
                while (frac_norm & 0x0400) == 0 {
                    frac_norm <<= 1;
                    exp32 -= 1;
                }
                // Remove the leading 1 bit
                frac_norm &= 0x03FF;
                let mant32 = (frac_norm) << 13;
                sign | ((exp32 as u32) << 23) | mant32
            }
        } else {
            // Normal case: just re-bias exponent and shift mantissa.
            let exp32 = ((exp) + (127 - 15) as u32) << 23; // +112
            let mant32 = frac << 13;
            sign | exp32 | mant32
        };

        f32::from_bits(f32_bits)
    }

    /// Convert from `f32` with IEEE-754 round-to-nearest, ties-to-even.
    ///
    /// Special cases:
    /// - NaN maps to a NaN (payload is truncated).
    /// - Overflow maps to ±infinity.
    /// - Subnormals are rounded to the nearest subnormal or zero.
    #[inline]
    pub fn from_f32(value: f32) -> Self {
        let bits = value.to_bits();

        let sign16 = ((bits >> 16) & 0x8000) as u16;
        let exponent32 = ((bits >> 23) & 0xFF) as i32;
        let mantissa32 = bits & 0x007F_FFFF;

        // Handle NaN / Inf
        if exponent32 == 0xFF {
            if mantissa32 == 0 {
                return F16(sign16 | 0x7C00); // ±Inf
            } else {
                // Preserve payload (truncate) and force a quiet NaN.
                let payload = (mantissa32 >> 13) as u16;
                // Ensure payload is non-zero to stay NaN after truncation.
                let payload = if payload == 0 { 1 } else { payload };
                return F16(sign16 | 0x7C00 | payload);
            }
        }

        // Normalized range mapping thresholds for binary16:
        //  - Max exponent before overflow: e32 > 142 -> ±Inf
        //  - Min normal: e32 >= 113
        //  - Subnormal zone: 103 <= e32 < 113
        //  - Underflow to zero: e32 < 103
        if exponent32 > 142 {
            // Overflow -> ±Inf
            return F16(sign16 | 0x7C00);
        }

        if exponent32 < 113 {
            // Might be subnormal or zero in half.
            if exponent32 < 103 {
                // Too small -> ±0
                return F16(sign16);
            }
            // Produce subnormal mantissa with correct RNTE rounding.
            // Restore the implicit leading 1 for f32 normals. For f32 subnormals
            // (exponent32 == 0), this still produces a correct scaling.
            let mant_with_hidden = mantissa32 | 0x0080_0000;

            let shift = (113 - exponent32) as u32; // 1 - e16
            let mant_trunc = mant_with_hidden >> (shift + 13);
            let guard = (mant_with_hidden >> (shift + 12)) & 1;
            let sticky_mask = (1u32 << (shift + 12)) - 1;
            let sticky = (mant_with_hidden & sticky_mask) != 0;

            let mut mant10 = mant_trunc as u16;
            let lsb = (mant10 & 1) != 0;
            if guard != 0 && (sticky || lsb) {
                mant10 = mant10.wrapping_add(1);
            }

            // Rounding can push the largest subnormal to the smallest normal.
            if mant10 == 0x0400 {
                // becomes +/− (1.0 * 2^-14)
                return F16(sign16 | 0x0400);
            }

            return F16(sign16 | mant10);
        }

        // Normal case: map exponent and mantissa, then RNTE on the 13 discarded bits.
        let exponent16 = (exponent32 - 112) as u16; // rebias: 127->15

        let mant_top10 = (mantissa32 >> 13) as u16;
        let guard = (mantissa32 >> 12) & 1;
        let sticky = (mantissa32 & 0xFFF) != 0;

        let mut mant10 = mant_top10;
        let lsb = (mant10 & 1) != 0;
        if guard != 0 && (sticky || lsb) {
            mant10 = mant10.wrapping_add(1);
        }

        let mut exp16 = exponent16;
        if mant10 == 0x0400 {
            // Mantissa overflow -> carry into exponent.
            mant10 = 0;
            exp16 += 1;
            if exp16 >= 0x1F {
                // Overflow after rounding -> ±Inf
                return F16(sign16 | 0x7C00);
            }
        }

        F16(sign16 | (exp16 << 10) | mant10)
    }
}

impl From<f32> for F16 {
    #[inline]
    fn from(value: f32) -> Self {
        F16::from_f32(value)
    }
}

impl From<F16> for f32 {
    #[inline]
    fn from(value: F16) -> Self {
        value.to_f32()
    }
}

/// bfloat16 ("Brain floating point 16") stored as raw 16-bit bits.
///
/// Layout: 1 sign bit, 8 exponent bits (bias 127, identical to `f32`), 7 fraction bits.
///
/// This type is `#[repr(transparent)]` over `u16` and therefore has the exact
/// same size and alignment as `u16`. No heap allocations are performed.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BF16(u16);

impl core::fmt::Debug for BF16 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "BF16 {{ value: {}, bits: 0x{:04X} }}",
            self.to_f32(),
            self.0
        )
    }
}

impl BF16 {
    /// Construct from raw bits (bit pattern is preserved).
    #[inline]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// Return the raw 16-bit representation.
    #[inline]
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    /// True if the number is NaN.
    #[inline]
    pub const fn is_nan(self) -> bool {
        let exp = (self.0 >> 7) & 0xFF;
        let frac = self.0 & 0x007F;
        exp == 0xFF && frac != 0
    }

    /// True if the number is ±infinity.
    #[inline]
    pub const fn is_infinite(self) -> bool {
        let exp = (self.0 >> 7) & 0xFF;
        let frac = self.0 & 0x007F;
        exp == 0xFF && frac == 0
    }

    /// True if the number is finite (not NaN or ±inf).
    #[inline]
    pub const fn is_finite(self) -> bool {
        ((self.0 >> 7) & 0xFF) != 0xFF
    }

    /// True if the sign bit is set (negative, including -0.0).
    #[inline]
    pub const fn is_sign_negative(self) -> bool {
        (self.0 & 0x8000) != 0
    }

    /// True if the sign bit is not set (positive, including +0.0).
    #[inline]
    pub const fn is_sign_positive(self) -> bool {
        (self.0 & 0x8000) == 0
    }

    /// Convert to `f32` by widening (zero-extends the lower 16 bits).
    ///
    /// TODO: Check this.
    #[inline]
    pub fn to_f32(self) -> f32 {
        f32::from_bits((self.0 as u32) << 16)
    }

    /// Convert from `f32` with IEEE-754 round-to-nearest, ties-to-even.
    ///
    /// Implemented by adding a rounding bias to the truncated lower word.
    /// This preserves NaN payloads (truncated) and maps overflow to ±inf.
    #[inline]
    pub fn from_f32(value: f32) -> Self {
        let bits = value.to_bits();
        // Add 0x7FFF + LSB of the upper half for ties-to-even.
        let lsb = (bits >> 16) & 1;
        let rounded = bits.wrapping_add(0x7FFF + lsb);
        let mut upper = (rounded >> 16) as u16;

        // Preserve NaN (ensure mantissa != 0 when exponent is all 1s and original had mantissa).
        if ((upper & 0x7F80) == 0x7F80) && ((bits & 0x007F_FFFF) != 0) && ((upper & 0x007F) == 0) {
            upper |= 0x0001;
        }

        BF16(upper)
    }
}

impl From<f32> for BF16 {
    #[inline]
    fn from(value: f32) -> Self {
        BF16::from_f32(value)
    }
}

impl From<BF16> for f32 {
    #[inline]
    fn from(value: BF16) -> Self {
        value.to_f32()
    }
}

impl DType for bool {
    const DATA_TYPE: DataType = DataType::Boolean;
}

impl DType for u8 {
    const DATA_TYPE: DataType = DataType::Unsigned8;
}

impl DType for u16 {
    const DATA_TYPE: DataType = DataType::Unsigned16;
}

impl DType for u32 {
    const DATA_TYPE: DataType = DataType::Unsigned32;
}

impl DType for u64 {
    const DATA_TYPE: DataType = DataType::Unsigned64;
}

impl DType for i8 {
    const DATA_TYPE: DataType = DataType::Signed8;
}

impl DType for i16 {
    const DATA_TYPE: DataType = DataType::Signed16;
}

impl DType for i32 {
    const DATA_TYPE: DataType = DataType::Signed32;
}

impl DType for i64 {
    const DATA_TYPE: DataType = DataType::Signed64;
}

impl DType for f32 {
    const DATA_TYPE: DataType = DataType::Float32;
}

impl DType for f64 {
    const DATA_TYPE: DataType = DataType::Float64;
}

impl DType for F16 {
    const DATA_TYPE: DataType = DataType::Float16;
}

impl DType for BF16 {
    const DATA_TYPE: DataType = DataType::BFloat16;
}
