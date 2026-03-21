use lru::LruCache;
use rustler::{Binary, Encoder, Env, Term};
use std::num::NonZeroUsize;
use std::sync::{Mutex, OnceLock};
use unicode_normalization::UnicodeNormalization;

mod atoms {
    rustler::atoms! {
        ok,
        error,
    }
}

/// Default LRU cache capacity per function (configurable at load time).
const DEFAULT_CACHE_SIZE: usize = 10_000;

/// Result type stored in cache: Some(bytes) = success, None = error.
type CacheValue = Option<Vec<u8>>;

/// Per-function LRU caches. Each function gets its own mutex to avoid
/// contention between different prep profiles.
struct PrepCaches {
    nodeprep: Mutex<LruCache<Vec<u8>, CacheValue>>,
    nameprep: Mutex<LruCache<Vec<u8>, CacheValue>>,
    resourceprep: Mutex<LruCache<Vec<u8>, CacheValue>>,
    tolower_nofilter: Mutex<LruCache<Vec<u8>, CacheValue>>,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

impl PrepCaches {
    fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1).unwrap());
        PrepCaches {
            nodeprep: Mutex::new(LruCache::new(cap)),
            nameprep: Mutex::new(LruCache::new(cap)),
            resourceprep: Mutex::new(LruCache::new(cap)),
            tolower_nofilter: Mutex::new(LruCache::new(cap)),
            hits: Mutex::new(0),
            misses: Mutex::new(0),
        }
    }

    fn record_hit(&self) {
        if let Ok(mut h) = self.hits.lock() {
            *h += 1;
        }
    }

    fn record_miss(&self) {
        if let Ok(mut m) = self.misses.lock() {
            *m += 1;
        }
    }
}

static CACHES: OnceLock<PrepCaches> = OnceLock::new();

fn caches() -> &'static PrepCaches {
    CACHES.get_or_init(|| PrepCaches::new(DEFAULT_CACHE_SIZE))
}

// ---------------------------------------------------------------------------
// Helper: create an Erlang binary term from a byte slice
// ---------------------------------------------------------------------------

fn make_binary_term<'a>(env: Env<'a>, data: &[u8]) -> Term<'a> {
    let mut bin = rustler::OwnedBinary::new(data.len()).expect("binary allocation failed");
    bin.as_mut_slice().copy_from_slice(data);
    bin.release(env).encode(env)
}

// ---------------------------------------------------------------------------
// Cached wrapper: look up in cache, compute on miss, store result
// ---------------------------------------------------------------------------

fn cached_prep<'a, F>(
    env: Env<'a>,
    input: &[u8],
    cache: &Mutex<LruCache<Vec<u8>, CacheValue>>,
    compute: F,
) -> Term<'a>
where
    F: FnOnce(&str) -> CacheValue,
{
    let key = input.to_vec();

    // Fast path: empty input → empty binary
    if input.is_empty() {
        return make_binary_term(env, &[]);
    }

    // Check cache
    if let Ok(mut c) = cache.lock()
        && let Some(result) = c.get(&key)
    {
        caches().record_hit();
        return match result {
            Some(bytes) => make_binary_term(env, bytes),
            None => atoms::error().encode(env),
        };
    }

    caches().record_miss();

    // Validate UTF-8 first — invalid UTF-8 always returns error
    let s = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(_) => {
            if let Ok(mut c) = cache.lock() {
                c.put(key, None);
            }
            return atoms::error().encode(env);
        }
    };

    // Compute the result
    let result = compute(s);

    // Store in cache
    let ret = match &result {
        Some(bytes) => make_binary_term(env, bytes),
        None => atoms::error().encode(env),
    };

    if let Ok(mut c) = cache.lock() {
        c.put(key, result);
    }

    ret
}

// ---------------------------------------------------------------------------
// NIF functions
// ---------------------------------------------------------------------------

#[rustler::nif(name = "nodeprep_nif")]
fn nodeprep<'a>(env: Env<'a>, input: Binary) -> Term<'a> {
    cached_prep(env, input.as_slice(), &caches().nodeprep, |s| {
        stringprep::nodeprep(s)
            .ok()
            .map(|cow| cow.into_owned().into_bytes())
    })
}

#[rustler::nif(name = "nameprep_nif")]
fn nameprep<'a>(env: Env<'a>, input: Binary) -> Term<'a> {
    cached_prep(env, input.as_slice(), &caches().nameprep, |s| {
        stringprep::nameprep(s)
            .ok()
            .map(|cow| cow.into_owned().into_bytes())
    })
}

#[rustler::nif(name = "resourceprep_nif")]
fn resourceprep<'a>(env: Env<'a>, input: Binary) -> Term<'a> {
    cached_prep(env, input.as_slice(), &caches().resourceprep, |s| {
        stringprep::resourceprep(s)
            .ok()
            .map(|cow| cow.into_owned().into_bytes())
    })
}

/// tolower is identical to nameprep in the C implementation
/// (same prohibit mask ACMask, same toLower=true).
/// We share the nameprep cache since results are identical.
#[rustler::nif(name = "tolower_nif")]
fn tolower<'a>(env: Env<'a>, input: Binary) -> Term<'a> {
    cached_prep(env, input.as_slice(), &caches().nameprep, |s| {
        stringprep::nameprep(s)
            .ok()
            .map(|cow| cow.into_owned().into_bytes())
    })
}

/// tolower_nofilter: case folding + NFC normalization + B.1 removal,
/// without character prohibition checks. Bidi validation still applies.
///
/// The C implementation uses prohibit=0 (no character prohibition) but
/// still applies the full stringprep pipeline: B.1 removal, B.2 case
/// folding, NFD decomposition, canonical reordering, NFC composition,
/// and bidirectional text validation.
///
/// We approximate this using:
/// 1. str::to_lowercase() for Unicode case folding
/// 2. NFC normalization via unicode-normalization crate
/// 3. B.1 removal (zero-width spaces, soft hyphens, etc.)
///
/// Note: str::to_lowercase() uses full Unicode case mapping which is
/// close to but not identical to stringprep B.2 tables (which are based
/// on Unicode 3.2). For production XMPP use this is sufficient since
/// tolower_nofilter is only used for loose comparison, not for protocol
/// identity.
#[rustler::nif(name = "tolower_nofilter_nif")]
fn tolower_nofilter<'a>(env: Env<'a>, input: Binary) -> Term<'a> {
    cached_prep(env, input.as_slice(), &caches().tolower_nofilter, |s| {
        // Apply B.1 removal (commonly mapped to nothing)
        let filtered: String = s.chars().filter(|c| !is_b1_char(*c)).collect();

        // Apply case folding (lowercasing)
        let lowered = filtered.to_lowercase();

        // Apply NFC normalization
        let normalized: String = lowered.nfc().collect();

        // Bidi check
        if !check_bidi(&normalized) {
            return None;
        }

        Some(normalized.into_bytes())
    })
}

/// RFC 3454 Table B.1 — Characters commonly mapped to nothing.
/// These are removed during stringprep processing.
fn is_b1_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'   // SOFT HYPHEN
        | '\u{1806}' // MONGOLIAN TODO SOFT HYPHEN
        | '\u{200B}' // ZERO WIDTH SPACE
        | '\u{2060}' // WORD JOINER
        | '\u{FEFF}' // ZERO WIDTH NO-BREAK SPACE (BOM)
        | '\u{034F}' // COMBINING GRAPHEME JOINER
        | '\u{180B}' // MONGOLIAN FREE VARIATION SELECTOR ONE
        | '\u{180C}' // MONGOLIAN FREE VARIATION SELECTOR TWO
        | '\u{180D}' // MONGOLIAN FREE VARIATION SELECTOR THREE
        | '\u{200C}' // ZERO WIDTH NON-JOINER
        | '\u{200D}' // ZERO WIDTH JOINER
        | '\u{FE00}'..='\u{FE0F}' // VARIATION SELECTORS 1-16
    )
}

/// Simplified bidirectional text check (RFC 3454 Section 6).
/// If the string contains any RandALCat characters, it must start
/// and end with RandALCat characters, and must not contain any LCat
/// characters.
fn check_bidi(s: &str) -> bool {
    let mut has_ral = false;
    let mut has_l = false;
    let mut first_ral = None;
    let mut last_ral = false;

    for c in s.chars() {
        let is_ral = is_ral_char(c);
        let is_l = is_l_char(c);

        if first_ral.is_none() {
            first_ral = Some(is_ral);
        }
        last_ral = is_ral;
        has_ral = has_ral || is_ral;
        has_l = has_l || is_l;
    }

    if has_ral {
        // Must start and end with RandALCat, and must not contain LCat
        first_ral.unwrap_or(false) && last_ral && !has_l
    } else {
        true
    }
}

/// Check if a character is in the RandALCat (RFC 3454 Table D.1).
/// Covers Arabic, Hebrew, Syriac, Thaana, and related RTL scripts.
fn is_ral_char(c: char) -> bool {
    matches!(
        c as u32,
        0x05BE
        | 0x05C0
        | 0x05C3
        | 0x05D0..=0x05EA
        | 0x05F0..=0x05F4
        | 0x061B
        | 0x061F
        | 0x0621..=0x063A
        | 0x0640..=0x064A
        | 0x066D..=0x066F
        | 0x0671..=0x06D5
        | 0x06DD
        | 0x06E5..=0x06E6
        | 0x06FA..=0x06FE
        | 0x0700..=0x070D
        | 0x0710
        | 0x0712..=0x072C
        | 0x0780..=0x07A5
        | 0x07B1
        | 0xFB1D
        | 0xFB1F..=0xFB28
        | 0xFB2A..=0xFB36
        | 0xFB38..=0xFB3C
        | 0xFB3E
        | 0xFB40..=0xFB41
        | 0xFB43..=0xFB44
        | 0xFB46..=0xFBB1
        | 0xFBD3..=0xFD3D
        | 0xFD50..=0xFD8F
        | 0xFD92..=0xFDC7
        | 0xFDF0..=0xFDFC
        | 0xFE70..=0xFE74
        | 0xFE76..=0xFEFC
    )
}

/// Check if a character is in the LCat (RFC 3454 Table D.2).
/// Simplified check: characters with Unicode Bidi_Class L.
fn is_l_char(c: char) -> bool {
    // Simplified: ASCII letters and most Latin/Greek/Cyrillic are LCat.
    // For full compliance, a proper Unicode bidi class lookup is needed.
    // This covers the common cases relevant to XMPP JID processing.
    let cp = c as u32;
    matches!(
        cp,
        0x0041..=0x005A // A-Z
        | 0x0061..=0x007A // a-z
        | 0x00AA
        | 0x00B5
        | 0x00BA
        | 0x00C0..=0x00D6
        | 0x00D8..=0x00F6
        | 0x00F8..=0x0220
        | 0x0222..=0x0233
        | 0x0250..=0x02AD
        | 0x02B0..=0x02B8
        | 0x02BB..=0x02C1
        | 0x02D0..=0x02D1
        | 0x02E0..=0x02E4
        | 0x02EE
        | 0x037A
        | 0x0386
        | 0x0388..=0x038A
        | 0x038C
        | 0x038E..=0x03A1
        | 0x03A3..=0x03CE
        | 0x03D0..=0x03F5
        | 0x0400..=0x0482
        | 0x048A..=0x04CE
        | 0x04D0..=0x04F5
        | 0x04F8..=0x04F9
        | 0x0500..=0x050F
        | 0x0531..=0x0556
        | 0x0559..=0x055F
        | 0x0561..=0x0587
        | 0x0589
        | 0x0903
        | 0x0905..=0x0939
        | 0x093D..=0x0940
        | 0x0949..=0x094C
        | 0x0950
        | 0x0958..=0x0961
        | 0x0964..=0x0970
        | 0x0982..=0x0983
        | 0x0985..=0x098C
        | 0x098F..=0x0990
        | 0x0993..=0x09A8
        | 0x09AA..=0x09B0
        | 0x09B2
        | 0x09B6..=0x09B9
        | 0x09BE..=0x09C0
        | 0x09C7..=0x09C8
        | 0x09CB..=0x09CC
        | 0x09D7
        | 0x09DC..=0x09DD
        | 0x09DF..=0x09E1
        | 0x09E6..=0x09F1
        | 0x09F4..=0x09FA
        | 0x0A05..=0x0A0A
        | 0x0A0F..=0x0A10
        | 0x0A13..=0x0A28
        | 0x0A2A..=0x0A30
        | 0x0A32..=0x0A33
        | 0x0A35..=0x0A36
        | 0x0A38..=0x0A39
        | 0x0A3E..=0x0A40
        | 0x0A59..=0x0A5C
        | 0x0A5E
        | 0x0A66..=0x0A6F
        | 0x0A72..=0x0A74
        | 0x0A83
        | 0x0A85..=0x0A8B
        | 0x0A8D
        | 0x0A8F..=0x0A91
        | 0x0A93..=0x0AA8
        | 0x0AAA..=0x0AB0
        | 0x0AB2..=0x0AB3
        | 0x0AB5..=0x0AB9
        | 0x0ABD..=0x0AC0
        | 0x0AC9
        | 0x0ACB..=0x0ACC
        | 0x0AD0
        | 0x0AE0
        | 0x0AE6..=0x0AEF
        | 0x0B02..=0x0B03
        | 0x0B05..=0x0B0C
        | 0x0B0F..=0x0B10
        | 0x0B13..=0x0B28
        | 0x0B2A..=0x0B30
        | 0x0B32..=0x0B33
        | 0x0B36..=0x0B39
        | 0x0B3D..=0x0B3E
        | 0x0B40
        | 0x0B47..=0x0B48
        | 0x0B4B..=0x0B4C
        | 0x0B57
        | 0x0B5C..=0x0B5D
        | 0x0B5F..=0x0B61
        | 0x0B66..=0x0B70
        | 0x0B83
        | 0x0B85..=0x0B8A
        | 0x0B8E..=0x0B90
        | 0x0B92..=0x0B95
        | 0x0B99..=0x0B9A
        | 0x0B9C
        | 0x0B9E..=0x0B9F
        | 0x0BA3..=0x0BA4
        | 0x0BA8..=0x0BAA
        | 0x0BAE..=0x0BB5
        | 0x0BB7..=0x0BB9
        | 0x0BBE..=0x0BBF
        | 0x0BC1..=0x0BC2
        | 0x0BC6..=0x0BC8
        | 0x0BCA..=0x0BCC
        | 0x0BD7
        | 0x0BE7..=0x0BF2
        | 0x0C01..=0x0C03
        | 0x0C05..=0x0C0C
        | 0x0C0E..=0x0C10
        | 0x0C12..=0x0C28
        | 0x0C2A..=0x0C33
        | 0x0C35..=0x0C39
        | 0x0C41..=0x0C44
        | 0x0C60..=0x0C61
        | 0x0C66..=0x0C6F
        | 0x0C82..=0x0C83
        | 0x0C85..=0x0C8C
        | 0x0C8E..=0x0C90
        | 0x0C92..=0x0CA8
        | 0x0CAA..=0x0CB3
        | 0x0CB5..=0x0CB9
        | 0x0CBE
        | 0x0CC0..=0x0CC4
        | 0x0CC7..=0x0CC8
        | 0x0CCA..=0x0CCB
        | 0x0CD5..=0x0CD6
        | 0x0CDE
        | 0x0CE0..=0x0CE1
        | 0x0CE6..=0x0CEF
        | 0x0D02..=0x0D03
        | 0x0D05..=0x0D0C
        | 0x0D0E..=0x0D10
        | 0x0D12..=0x0D28
        | 0x0D2A..=0x0D39
        | 0x0D3E..=0x0D40
        | 0x0D46..=0x0D48
        | 0x0D4A..=0x0D4C
        | 0x0D57
        | 0x0D60..=0x0D61
        | 0x0D66..=0x0D6F
        | 0x0D82..=0x0D83
        | 0x0D85..=0x0D96
        | 0x0D9A..=0x0DB1
        | 0x0DB3..=0x0DBB
        | 0x0DBD
        | 0x0DC0..=0x0DC6
        | 0x0DCF..=0x0DD1
        | 0x0DD8..=0x0DDF
        | 0x0DF2..=0x0DF4
        | 0x0E01..=0x0E30
        | 0x0E32..=0x0E33
        | 0x0E40..=0x0E46
        | 0x0E4F..=0x0E5B
        | 0x0E81..=0x0E82
        | 0x0E84
        | 0x0E87..=0x0E88
        | 0x0E8A
        | 0x0E8D
        | 0x0E94..=0x0E97
        | 0x0E99..=0x0E9F
        | 0x0EA1..=0x0EA3
        | 0x0EA5
        | 0x0EA7
        | 0x0EAA..=0x0EAB
        | 0x0EAD..=0x0EB0
        | 0x0EB2..=0x0EB3
        | 0x0EBD
        | 0x0EC0..=0x0EC4
        | 0x0EC6
        | 0x0ED0..=0x0ED9
        | 0x0EDC..=0x0EDD
        | 0x0F00..=0x0F17
        | 0x0F1A..=0x0F34
        | 0x0F36
        | 0x0F38
        | 0x0F3E..=0x0F47
        | 0x0F49..=0x0F6A
        | 0x0F7F
        | 0x0F85
        | 0x0F88..=0x0F8B
        | 0x0FBE..=0x0FC5
        | 0x0FC7..=0x0FCC
        | 0x0FCF
        | 0x1000..=0x1021
        | 0x1023..=0x1027
        | 0x1029..=0x102A
        | 0x102C
        | 0x1031
        | 0x1038
        | 0x1040..=0x1057
        | 0x10A0..=0x10C5
        | 0x10D0..=0x10F8
        | 0x10FB
        | 0x1100..=0x1159
        | 0x115F..=0x11A2
        | 0x11A8..=0x11F9
        | 0x1200..=0x1206
        | 0x1208..=0x1246
        | 0x1248
        | 0x124A..=0x124D
        | 0x1250..=0x1256
        | 0x1258
        | 0x125A..=0x125D
        | 0x1260..=0x1286
        | 0x1288
        | 0x128A..=0x128D
        | 0x1290..=0x12AE
        | 0x12B0
        | 0x12B2..=0x12B5
        | 0x12B8..=0x12BE
        | 0x12C0
        | 0x12C2..=0x12C5
        | 0x12C8..=0x12CE
        | 0x12D0..=0x12D6
        | 0x12D8..=0x12EE
        | 0x12F0..=0x130E
        | 0x1310
        | 0x1312..=0x1315
        | 0x1318..=0x131E
        | 0x1320..=0x1346
        | 0x1348..=0x135A
        | 0x1361..=0x137C
        | 0x13A0..=0x13F4
        | 0x1401..=0x1676
        | 0x1681..=0x169A
        | 0x16A0..=0x16F0
        | 0x1700..=0x170C
        | 0x170E..=0x1711
        | 0x1720..=0x1731
        | 0x1735..=0x1736
        | 0x1740..=0x1751
        | 0x1760..=0x176C
        | 0x176E..=0x1770
        | 0x1780..=0x17B6
        | 0x17BE..=0x17C5
        | 0x17C7..=0x17C8
        | 0x17D4..=0x17DA
        | 0x17DC
        | 0x17E0..=0x17E9
        | 0x1810..=0x1819
        | 0x1820..=0x1877
        | 0x1880..=0x18A8
        | 0x1E00..=0x1E9B
        | 0x1EA0..=0x1EF9
        | 0x1F00..=0x1F15
        | 0x1F18..=0x1F1D
        | 0x1F20..=0x1F45
        | 0x1F48..=0x1F4D
        | 0x1F50..=0x1F57
        | 0x1F59
        | 0x1F5B
        | 0x1F5D
        | 0x1F5F..=0x1F7D
        | 0x1F80..=0x1FB4
        | 0x1FB6..=0x1FBC
        | 0x1FBE
        | 0x1FC2..=0x1FC4
        | 0x1FC6..=0x1FCC
        | 0x1FD0..=0x1FD3
        | 0x1FD6..=0x1FDB
        | 0x1FE0..=0x1FEC
        | 0x1FF2..=0x1FF4
        | 0x1FF6..=0x1FFC
        | 0x200E
        | 0x2071
        | 0x207F
        | 0x2102
        | 0x2107
        | 0x210A..=0x2113
        | 0x2115
        | 0x2119..=0x211D
        | 0x2124
        | 0x2126
        | 0x2128
        | 0x212A..=0x212D
        | 0x212F..=0x2131
        | 0x2133..=0x2139
        | 0x213D..=0x213F
        | 0x2145..=0x2149
        | 0x2160..=0x2183
        | 0x2336..=0x237A
        | 0x2395
        | 0x249C..=0x24E9
        | 0x3005..=0x3007
        | 0x3021..=0x3029
        | 0x3031..=0x3035
        | 0x3038..=0x303C
        | 0x3041..=0x3096
        | 0x309D..=0x309F
        | 0x30A1..=0x30FA
        | 0x30FC..=0x30FF
        | 0x3105..=0x312C
        | 0x3131..=0x318E
        | 0x3190..=0x31B7
        | 0x31F0..=0x321C
        | 0x3220..=0x3243
        | 0x3260..=0x327B
        | 0x327F..=0x32B0
        | 0x32C0..=0x32CB
        | 0x32D0..=0x32FE
        | 0x3300..=0x3376
        | 0x337B..=0x33DD
        | 0x33E0..=0x33FE
        | 0x3400..=0x4DB5
        | 0x4E00..=0x9FA5
        | 0xA000..=0xA48C
        | 0xAC00..=0xD7A3
        | 0xD800..=0xFA2D
        | 0xFA30..=0xFA6A
        | 0xFB00..=0xFB06
        | 0xFB13..=0xFB17
        | 0xFF21..=0xFF3A
        | 0xFF41..=0xFF5A
        | 0xFF66..=0xFFBE
        | 0xFFC2..=0xFFC7
        | 0xFFCA..=0xFFCF
        | 0xFFD2..=0xFFD7
        | 0xFFDA..=0xFFDC
        | 0x10300..=0x1031E
        | 0x10320..=0x10323
        | 0x10330..=0x1034A
        | 0x10400..=0x10425
        | 0x10428..=0x1044D
        | 0x1D000..=0x1D0F5
        | 0x1D100..=0x1D126
        | 0x1D12A..=0x1D166
        | 0x1D16A..=0x1D172
        | 0x1D183..=0x1D184
        | 0x1D18C..=0x1D1A9
        | 0x1D1AE..=0x1D1DD
        | 0x1D400..=0x1D454
        | 0x1D456..=0x1D49C
        | 0x1D49E..=0x1D49F
        | 0x1D4A2
        | 0x1D4A5..=0x1D4A6
        | 0x1D4A9..=0x1D4AC
        | 0x1D4AE..=0x1D4B9
        | 0x1D4BB
        | 0x1D4BD..=0x1D4C0
        | 0x1D4C2..=0x1D4C3
        | 0x1D4C5..=0x1D505
        | 0x1D507..=0x1D50A
        | 0x1D50D..=0x1D514
        | 0x1D516..=0x1D51C
        | 0x1D51E..=0x1D539
        | 0x1D53B..=0x1D53E
        | 0x1D540..=0x1D544
        | 0x1D546
        | 0x1D54A..=0x1D550
        | 0x1D552..=0x1D6A3
        | 0x1D6A8..=0x1D7C9
        | 0x20000..=0x2A6D6
        | 0x2F800..=0x2FA1D
        | 0xF0000..=0xFFFFD
        | 0x100000..=0x10FFFD
    )
}

// ---------------------------------------------------------------------------
// Cache management NIFs
// ---------------------------------------------------------------------------

/// Returns total number of entries across all caches.
#[rustler::nif]
fn cache_size() -> u64 {
    let c = caches();
    let mut total = 0u64;
    if let Ok(cache) = c.nodeprep.lock() {
        total += cache.len() as u64;
    }
    if let Ok(cache) = c.nameprep.lock() {
        total += cache.len() as u64;
    }
    if let Ok(cache) = c.resourceprep.lock() {
        total += cache.len() as u64;
    }
    if let Ok(cache) = c.tolower_nofilter.lock() {
        total += cache.len() as u64;
    }
    total
}

/// Returns {Hits, Misses} cache statistics.
#[rustler::nif]
fn cache_stats() -> (u64, u64) {
    let c = caches();
    let hits = c.hits.lock().map(|h| *h).unwrap_or(0);
    let misses = c.misses.lock().map(|m| *m).unwrap_or(0);
    (hits, misses)
}

// ---------------------------------------------------------------------------
// NIF init
// ---------------------------------------------------------------------------

rustler::init!("stringprep_rust");
