//! Verifies that no heap block containing an original value is freed
//! without being wiped first.
//!
//! A counting global allocator scans every block on `dealloc` for canary
//! strings (ASCII, case-insensitive, so normalised lower-case keys are caught
//! too). The test is a single `#[test]` so no other test frees canaries
//! concurrently.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use vault::{RevealMode, Span, Vault};

const CANARIES: &[&[u8]] =
    &[b"de44 5001 0517 5407 3249 31", b"zebrafischxq quokkawurm", b"canary.k7@example.org", b"de44500105175407324931"];

static ARMED: AtomicBool = AtomicBool::new(false);
static LEAKS: AtomicUsize = AtomicUsize::new(0);

struct Scanning;

fn contains_canary(block: &[u8]) -> bool {
    CANARIES.iter().any(|c| block.windows(c.len()).any(|w| w.eq_ignore_ascii_case(c)))
}

// SAFETY: delegates to the system allocator; the scan only reads memory of a
// block that is still allocated.
unsafe impl GlobalAlloc for Scanning {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
            if contains_canary(block) {
                LEAKS.fetch_add(1, Ordering::SeqCst);
            }
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Scanning = Scanning;

fn armed<R>(f: impl FnOnce() -> R) -> (R, usize) {
    LEAKS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let r = f();
    ARMED.store(false, Ordering::SeqCst);
    (r, LEAKS.load(Ordering::SeqCst))
}

#[test]
fn originals_are_wiped_before_free() {
    // Positive control: a plain String with a canary is detected when freed.
    let ((), leaks) = armed(|| drop(String::from("Zebrafischxq Quokkawurm")));
    assert_eq!(leaks, 1, "allocator scan must detect an unwiped canary");

    // The input lives in static memory, so only vault-owned copies are on the heap.
    const TEXT: &str = "Zebrafischxq Quokkawurm (canary.k7@example.org) zahlt von DE44 5001 0517 5407 3249 31. \
                        Nochmal: zebrafischxq   QUOKKAWURM.";
    let ((), leaks) = armed(|| {
        let mut vault = Vault::new();
        let find = |needle: &str| TEXT.find(needle).unwrap();
        let p = find("Zebrafischxq Quokkawurm");
        let e = find("canary.k7@example.org");
        let i = find("DE44 5001");
        let p2 = find("zebrafischxq   QUOKKAWURM");
        let spans = [
            Span { start: p, end: p + 23, kind: "PERSON" },
            Span { start: e, end: e + 21, kind: "EMAIL" },
            Span { start: i, end: i + 27, kind: "IBAN" },
            Span { start: p2, end: p2 + 25, kind: "PERSON" },
        ];
        let pseudo = vault.pseudonymize(TEXT, spans).unwrap();
        assert_eq!(pseudo.matches("[PERSON_1]").count(), 2);
        assert!(!contains_canary(pseudo.as_bytes()));

        // Many more entries force the internal maps and vectors to grow.
        for n in 0..200 {
            let name = format!("Person Nummer {n}");
            vault.placeholder("PERSON", &name).unwrap();
        }

        let revealed = vault.reveal_with(&pseudo, RevealMode::Lenient);
        assert!(contains_canary(revealed.text.as_bytes()));
        assert_eq!(revealed.restored, 4);
        drop(revealed);
        drop(pseudo);
        vault.clear();
        vault.placeholder("PERSON", "Zebrafischxq Quokkawurm").unwrap();
        drop(vault);
    });
    assert_eq!(leaks, 0, "{leaks} heap block(s) with original values were freed without zeroization");
}
