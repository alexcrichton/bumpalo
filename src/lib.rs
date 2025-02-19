/*!

**A fast bump allocation arena for Rust.**

[![](https://docs.rs/bumpalo/badge.svg)](https://docs.rs/bumpalo/)
[![](https://img.shields.io/crates/v/bumpalo.svg)](https://crates.io/crates/bumpalo)
[![](https://img.shields.io/crates/d/bumpalo.svg)](https://crates.io/crates/bumpalo)
[![Travis CI Build Status](https://travis-ci.org/fitzgen/bumpalo.svg?branch=master)](https://travis-ci.org/fitzgen/bumpalo)

![](https://github.com/fitzgen/bumpalo/raw/master/bumpalo.png)

## Bump Allocation

Bump allocation is a fast, but limited approach to allocation. We have a chunk
of memory, and we maintain a pointer within that memory. Whenever we allocate an
object, we do a quick test that we have enough capacity left in our chunk to
allocate the object and then increment the pointer by the object's size. *That's
it!*

The disadvantage of bump allocation is that there is no general way to
deallocate individual objects or reclaim the memory region for a
no-longer-in-use object.

These trade offs make bump allocation well-suited for *phase-oriented*
allocations. That is, a group of objects that will all be allocated during the
same program phase, used, and then can all be deallocated together as a group.

## Deallocation en Masse, but No `Drop`

To deallocate all the objects in the arena at once, we can simply reset the bump
pointer back to the start of the arena's memory chunk. This makes mass
deallocation *extremely* fast, but allocated objects' `Drop` implementations are
not invoked.

## What happens when the memory chunk is full?

This implementation will allocate a new memory chunk from the global allocator
and then start bump allocating into this new memory chunk.

## Example

```
use bumpalo::Bump;
use std::u64;

struct Doggo {
    cuteness: u64,
    age: u8,
    scritches_required: bool,
}

// Create a new arena to bump allocate into.
let bump = Bump::new();

// Allocate values into the arena.
let scooter = bump.alloc(Doggo {
    cuteness: u64::max_value(),
    age: 8,
    scritches_required: true,
});

assert!(scooter.scritches_required);
```

## Collections

When the on-by-default `"collections"` feature is enabled, a fork of some of the
`std` library's collections are available in the `collections` module. These
collection types are modified to allocate their space inside `bumpalo::Bump`
arenas.

```rust
use bumpalo::{Bump, collections::Vec};

// Create a new bump arena.
let bump = Bump::new();

// Create a vector of integers whose storage is backed by the bump arena. The
// vector cannot outlive its backing arena, and this property is enforced with
// Rust's lifetime rules.
let mut v = Vec::new_in(&bump);

// Push a bunch of integers onto `v`!
for i in 0..100 {
    v.push(i);
}
```

Eventually [all `std` collection types will be parameterized by an
allocator](https://github.com/rust-lang/rust/issues/42774) and we can remove
this `collections` module and use the `std` versions.

## `#![no_std]` Support

Requires the `alloc` nightly feature. Disable the on-by-default `"std"` feature:

```toml
[dependencies.bumpalo]
version = "1"
default-features = false
```

 */

#![deny(missing_debug_implementations)]
#![deny(missing_docs)]
// In no-std mode, use the alloc crate to get `Vec`.
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(feature = "std"), feature(alloc))]

#[cfg(feature = "std")]
extern crate core;

#[cfg(feature = "collections")]
pub mod collections;

mod alloc;

#[cfg(feature = "std")]
mod imports {
    pub use std::alloc::{alloc, dealloc, Layout};
    pub use std::cell::{Cell, UnsafeCell};
    pub use std::cmp;
    pub use std::fmt;
    pub use std::mem;
    pub use std::ptr::{self, NonNull};
    pub use std::slice;
}

#[cfg(not(feature = "std"))]
mod imports {
    extern crate alloc;
    pub use self::alloc::alloc::{alloc, dealloc, Layout};
    pub use core::cell::{Cell, UnsafeCell};
    pub use core::cmp;
    pub use core::fmt;
    pub use core::mem;
    pub use core::ptr::{self, NonNull};
    pub use core::slice;
}

use crate::imports::*;

/// An arena to bump allocate into.
///
/// ## No `Drop`s
///
/// Objects that are bump-allocated will never have their `Drop` implementation
/// called &mdash; unless you do it manually yourself. This makes it relatively
/// easy to leak memory or other resources.
///
/// If you have a type which internally manages
///
/// * an allocation from the global heap (e.g. `Vec<T>`),
/// * open file descriptors (e.g. `std::fs::File`), or
/// * any other resource that must be cleaned up (e.g. an `mmap`)
///
/// and relies on its `Drop` implementation to clean up the internal resource,
/// then if you allocate that type with a `Bump`, you need to find a new way to
/// clean up after it yourself.
///
/// Potential solutions are
///
/// * calling [`drop_in_place`][drop_in_place] or using
///   [`std::mem::ManuallyDrop`][manuallydrop] to manually drop these types,
/// * using `bumpalo::collections::Vec` instead of `std::vec::Vec`, or
/// * simply avoiding allocating these problematic types within a `Bump`.
///
/// Note that not calling `Drop` is memory safe! Destructors are never
/// guaranteed to run in Rust, you can't rely on them for enforcing memory
/// safety.
///
/// [drop_in_place]: https://doc.rust-lang.org/stable/std/ptr/fn.drop_in_place.html
/// [manuallydrop]: https://doc.rust-lang.org/stable/std/mem/struct.ManuallyDrop.html
///
/// ## Example
///
/// ```
/// use bumpalo::Bump;
///
/// // Create a new bump arena.
/// let bump = Bump::new();
///
/// // Allocate values into the arena.
/// let forty_two = bump.alloc(42);
/// assert_eq!(*forty_two, 42);
///
/// // Mutable references are returned from allocation.
/// let mut s = bump.alloc("bumpalo");
/// *s = "the bump allocator; and also is a buffalo";
/// ```
#[derive(Debug)]
pub struct Bump {
    // The current chunk we are bump allocating within.
    current_chunk_footer: Cell<NonNull<ChunkFooter>>,

    // The first chunk we were ever given, which is the head of the intrusive
    // linked list of all chunks this arena has been bump allocating within.
    all_chunk_footers: Cell<NonNull<ChunkFooter>>,
}

#[repr(C)]
#[derive(Debug)]
struct ChunkFooter {
    // Pointer to the start of this chunk allocation. This footer is always at
    // the end of the chunk.
    data: NonNull<u8>,

    // The layout of this chunk's allocation.
    layout: Layout,

    // Link to the next chunk, if any.
    next: Cell<Option<NonNull<ChunkFooter>>>,

    // Bump allocation finger that is always in the range `self.data..=self`.
    ptr: Cell<NonNull<u8>>,
}

impl Drop for Bump {
    fn drop(&mut self) {
        unsafe {
            let mut footer = Some(self.all_chunk_footers.get());
            while let Some(f) = footer {
                footer = f.as_ref().next.get();
                dealloc(f.as_ref().data.as_ptr(), Bump::default_chunk_layout());
            }
        }
    }
}

#[inline]
pub(crate) fn round_up_to(n: usize, divisor: usize) -> usize {
    debug_assert!(divisor.is_power_of_two());
    (n + divisor - 1) & !(divisor - 1)
}

// Maximum typical overhead per allocation imposed by allocators.
const MALLOC_OVERHEAD: usize = 16;

// Choose a relatively small default initial chunk size, since we double chunk
// sizes as we grow bump arenas to amortize costs of hitting the global
// allocator.
const DEFAULT_CHUNK_SIZE_WITH_FOOTER: usize = (1 << 9) - MALLOC_OVERHEAD;
const DEFAULT_CHUNK_ALIGN: usize = mem::align_of::<ChunkFooter>();

/// Wrapper around `Layout::from_size_align` that adds debug assertions.
#[inline]
unsafe fn layout_from_size_align(size: usize, align: usize) -> Layout {
    if cfg!(debug_assertions) {
        Layout::from_size_align(size, align).unwrap()
    } else {
        Layout::from_size_align_unchecked(size, align)
    }
}

impl Bump {
    fn default_chunk_layout() -> Layout {
        unsafe { layout_from_size_align(DEFAULT_CHUNK_SIZE_WITH_FOOTER, DEFAULT_CHUNK_ALIGN) }
    }

    /// Construct a new arena to bump allocate into.
    ///
    /// ## Example
    ///
    /// ```
    /// let bump = bumpalo::Bump::new();
    /// # let _ = bump;
    /// ```
    pub fn new() -> Bump {
        let chunk_footer = Self::new_chunk(None);
        Bump {
            current_chunk_footer: Cell::new(chunk_footer),
            all_chunk_footers: Cell::new(chunk_footer),
        }
    }

    /// Allocate a new chunk and return its initialized footer.
    ///
    /// If given, `layouts` is a tuple of the current chunk layout and the
    /// layout of the allocation request that triggered us to fall back to
    /// allocating a new chunk of memory.
    fn new_chunk(layouts: Option<(Layout, Layout)>) -> NonNull<ChunkFooter> {
        unsafe {
            let layout: Layout =
                layouts.map_or_else(Bump::default_chunk_layout, |(old, requested)| {
                    let old_doubled = old.size().checked_mul(2).unwrap();
                    debug_assert_eq!(
                        old_doubled,
                        round_up_to(old_doubled, mem::align_of::<ChunkFooter>()),
                        "The old size was already a multiple of our chunk footer alignment, so no \
                         need to round it up again."
                    );

                    // Round the size up to a multiple of our footer's alignment so that
                    // we can be sure that our footer is properly aligned.
                    let requested_size =
                        round_up_to(requested.size(), mem::align_of::<ChunkFooter>());

                    let size = cmp::max(old_doubled, requested_size);
                    let align = cmp::max(old.align(), requested.align());
                    layout_from_size_align(size, align)
                });

            let size = layout.size();

            let data = alloc(layout);
            assert!(!data.is_null());
            let data = NonNull::new_unchecked(data);

            let next = Cell::new(None);
            let ptr = Cell::new(data);
            let footer_ptr = data.as_ptr() as usize + size - mem::size_of::<ChunkFooter>();
            let footer_ptr = footer_ptr as *mut ChunkFooter;
            ptr::write(
                footer_ptr,
                ChunkFooter {
                    data,
                    layout,
                    next,
                    ptr,
                },
            );
            NonNull::new_unchecked(footer_ptr)
        }
    }

    /// Reset this bump allocator.
    ///
    /// Performs mass deallocation on everything allocated in this arena by
    /// resetting the pointer into the underlying chunk of memory to the start
    /// of the chunk. Does not run any `Drop` implementations on deallocated
    /// objects; see [the `Bump` type's top-level
    /// documentation](./struct.Bump.html) for details.
    ///
    /// If this arena has allocated multiple chunks to bump allocate into, then
    /// the excess chunks are returned to the global allocator.
    ///
    /// ## Example
    ///
    /// ```
    /// let mut bump = bumpalo::Bump::new();
    ///
    /// // Allocate a bunch of things.
    /// {
    ///     for i in 0..100 {
    ///         bump.alloc(i);
    ///     }
    /// }
    ///
    /// // Reset the arena.
    /// bump.reset();
    ///
    /// // Allocate some new things in the space previously occupied by the
    /// // original things.
    /// for j in 200..400 {
    ///     bump.alloc(j);
    /// }
    ///```
    pub fn reset(&mut self) {
        // Takes `&mut self` so `self` must be unique and there can't be any
        // borrows active that would get invalidated by resetting.
        unsafe {
            let mut footer = Some(self.all_chunk_footers.get());

            // Reset the pointer in each of our chunks.
            while let Some(f) = footer {
                footer = f.as_ref().next.get();

                if f == self.current_chunk_footer.get() {
                    // If this is the current chunk, then reset the bump finger
                    // to the start of the chunk.
                    f.as_ref()
                        .ptr
                        .set(NonNull::new_unchecked(f.as_ref().data.as_ptr() as *mut u8));
                    f.as_ref().next.set(None);
                    self.all_chunk_footers.set(f);
                } else {
                    // If this is not the current chunk, return it to the global
                    // allocator.
                    dealloc(f.as_ref().data.as_ptr(), f.as_ref().layout.clone());
                }
            }

            debug_assert_eq!(
                self.all_chunk_footers.get(),
                self.current_chunk_footer.get(),
                "The current chunk should be the list head of all of our chunks"
            );
            debug_assert!(
                self.current_chunk_footer
                    .get()
                    .as_ref()
                    .next
                    .get()
                    .is_none(),
                "We should only have a single chunk"
            );
            debug_assert_eq!(
                self.current_chunk_footer.get().as_ref().ptr.get(),
                self.current_chunk_footer.get().as_ref().data,
                "Our chunk's bump finger should be reset to the start of its allocation"
            );
        }
    }

    /// Allocate an object in this `Bump` and return an exclusive reference to
    /// it.
    ///
    /// ## Panics
    ///
    /// Panics if reserving space for `T` would cause an overflow.
    ///
    /// ## Example
    ///
    /// ```
    /// let bump = bumpalo::Bump::new();
    /// let x = bump.alloc("hello");
    /// assert_eq!(*x, "hello");
    /// ```
    #[inline(always)]
    pub fn alloc<T>(&self, val: T) -> &mut T {
        let layout = Layout::new::<T>();

        unsafe {
            let p = self.alloc_layout(layout);
            let p = p.as_ptr() as *mut T;
            ptr::write(p, val);
            &mut *p
        }
    }

    /// Allocate space for an object with the given `Layout`.
    ///
    /// The returned pointer points at uninitialized memory, and should be
    /// initialized with
    /// [`std::ptr::write`](https://doc.rust-lang.org/stable/std/ptr/fn.write.html).
    ///
    /// ## Panics
    ///
    /// Panics if reserving space for `T` would cause an overflow.
    #[inline(always)]
    pub fn alloc_layout(&self, layout: Layout) -> NonNull<u8> {
        unsafe {
            let footer = self.current_chunk_footer.get();
            let footer = footer.as_ref();
            let ptr = footer.ptr.get().as_ptr() as usize;
            let ptr = round_up_to(ptr, layout.align());
            let end = footer as *const _ as usize;
            debug_assert!(ptr <= end);

            let new_ptr = match ptr.checked_add(layout.size()) {
                Some(p) => p,
                None => self.overflow(),
            };

            if new_ptr <= end {
                let p = ptr as *mut u8;
                debug_assert!(new_ptr <= footer as *const _ as usize);
                footer.ptr.set(NonNull::new_unchecked(new_ptr as *mut u8));
                return NonNull::new_unchecked(p);
            }
        }

        self.alloc_layout_slow(layout)
    }

    #[inline(never)]
    #[cold]
    fn overflow(&self) -> ! {
        panic!("allocation too large, caused overflow")
    }

    // Slow path allocation for when we need to allocate a new chunk from the
    // parent bump set because there isn't enough room in our current chunk.
    #[inline(never)]
    fn alloc_layout_slow(&self, layout: Layout) -> NonNull<u8> {
        unsafe {
            let size = layout.size();

            // Get a new chunk from the global allocator.
            let current_layout = self.current_chunk_footer.get().as_ref().layout.clone();
            let footer = Bump::new_chunk(Some((current_layout, layout)));

            // Set our current chunk's next link to this new chunk.
            self.current_chunk_footer
                .get()
                .as_ref()
                .next
                .set(Some(footer));

            // Set the new chunk as our new current chunk.
            self.current_chunk_footer.set(footer);

            // Move the bump ptr finger ahead to allocate room for `val`.
            let footer = footer.as_ref();
            let ptr = footer.ptr.get().as_ptr() as usize + size;
            debug_assert!(
                ptr <= footer as *const _ as usize,
                "{} <= {}",
                ptr,
                footer as *const _ as usize
            );
            footer.ptr.set(NonNull::new_unchecked(ptr as *mut u8));

            // Return a pointer to the start of this chunk.
            footer.data.cast::<u8>()
        }
    }

    /// Call `f` on each chunk of allocated memory that this arena has bump
    /// allocated into.
    ///
    /// `f` is invoked in order of allocation: oldest chunks first, newest
    /// chunks last.
    ///
    /// ## Safety
    ///
    /// Because this method takes `&mut self`, we know that the bump arena
    /// reference is unique and therefore there aren't any active references to
    /// any of the objects we've allocated in it either. This potential aliasing
    /// of exclusive references is one common footgun for unsafe code that we
    /// don't need to worry about here.
    ///
    /// However, there could be regions of uninitilized memory used as padding
    /// between allocations. Reading uninitialized memory is big time undefined
    /// behavior!
    ///
    /// The only way to guarantee that there is no padding between allocations
    /// or within allocated objects is if all of these properties hold:
    ///
    /// 1. Every object allocated in this arena has the same alignment.
    /// 2. Every object's size is a multiple of its alignment.
    /// 3. None of the objects allocated in this arena contain any internal
    ///    padding.
    ///
    /// If you want to use this `each_allocated_chunk` method, it is *your*
    /// responsibility to ensure that these properties hold!
    ///
    /// ## Example
    ///
    /// ```
    /// let mut bump = bumpalo::Bump::new();
    ///
    /// // Allocate a bunch of things in this bump arena, potentially causing
    /// // additional memory chunks to be reserved.
    /// for i in 0..10000 {
    ///     bump.alloc(i);
    /// }
    ///
    /// // Iterate over each chunk we've bump allocated into. This is safe
    /// // because we have only allocated `i32` objects in this arena.
    /// unsafe {
    ///     bump.each_allocated_chunk(|ch| {
    ///         println!("Used a chunk that is {} bytes long", ch.len());
    ///     });
    /// }
    /// ```
    pub unsafe fn each_allocated_chunk<F>(&mut self, mut f: F)
    where
        F: for<'a> FnMut(&'a [u8]),
    {
        let mut footer = Some(self.all_chunk_footers.get());
        while let Some(foot) = footer {
            let foot = foot.as_ref();

            let start = foot.data.as_ptr() as usize;
            let end_of_allocated_region = foot.ptr.get().as_ptr() as usize;
            debug_assert!(end_of_allocated_region <= foot as *const _ as usize);
            debug_assert!(
                end_of_allocated_region >= start,
                "end_of_allocated_region (0x{:x}) >= start (0x{:x})",
                end_of_allocated_region,
                start
            );

            let len = end_of_allocated_region - start;
            let slice = slice::from_raw_parts(start as *const u8, len);
            f(slice);

            footer = foot.next.get();
        }
    }
}

unsafe impl<'a> alloc::Alloc for &'a Bump {
    #[inline(always)]
    unsafe fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>, alloc::AllocErr> {
        Ok(self.alloc_layout(layout))
    }

    #[inline(always)]
    unsafe fn dealloc(&mut self, _ptr: NonNull<u8>, _layout: Layout) {}
}

#[test]
fn chunk_footer_is_three_words() {
    assert_eq!(mem::size_of::<ChunkFooter>(), mem::size_of::<usize>() * 5);
}
