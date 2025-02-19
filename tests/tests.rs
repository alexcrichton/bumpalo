extern crate bumpalo;

use bumpalo::Bump;
use std::alloc::Layout;
use std::mem;
use std::slice;
use std::usize;

#[test]
fn can_iterate_over_allocated_things() {
    let mut bump = Bump::new();

    const MAX: u64 = 131072;

    let mut chunks = vec![];
    let mut last = None;

    for i in 0..MAX {
        let this = bump.alloc(i);
        assert_eq!(*this, i);
        let this = this as *const _ as usize;

        if match last {
            Some(last) if last + mem::size_of::<u64>() == this => false,
            _ => true,
        } {
            println!("new chunk @ 0x{:x}", this);
            assert!(
                !chunks.contains(&this),
                "should not have already allocated this chunk"
            );
            chunks.push(this);
        }

        last = Some(this);
    }

    let mut seen = vec![false; MAX as usize];
    chunks.reverse();

    // Safe because we always allocated objects of the same type in this arena,
    // and their size >= their align.
    unsafe {
        bump.each_allocated_chunk(|ch| {
            let ch_usize = ch.as_ptr() as usize;
            println!("iter chunk @ 0x{:x}", ch_usize);
            assert_eq!(
                chunks.pop().unwrap(),
                ch_usize,
                "should iterate over each chunk once, in order they were allocated in"
            );

            let ch: &[u64] =
                slice::from_raw_parts(ch.as_ptr() as *mut u64, ch.len() / mem::size_of::<u64>());
            for i in ch {
                assert!(*i < MAX, "{} < {} (aka {:x} < {:x})", i, MAX, i, MAX);
                seen[*i as usize] = true;
            }
        });
    }

    assert!(seen.iter().all(|s| *s));
}

#[test]
#[should_panic(expected = "allocation too large, caused overflow")]
fn alloc_overflow() {
    let bump = Bump::new();
    let x = bump.alloc(0_u8);
    let p = x as *mut u8 as usize;

    // A size guaranteed to overflow.
    let size = usize::MAX - p + 1;
    let align = 1;
    let layout = match Layout::from_size_align(size, align) {
        Err(e) => {
            // Return on error so that we don't panic and the test fails.
            eprintln!("Layout::from_size_align errored: {}", e);
            return;
        }
        Ok(l) => l,
    };

    // This should panic.
    bump.alloc_layout(layout);
}
