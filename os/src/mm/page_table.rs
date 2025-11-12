//! Implementation of [`PageTableEntry`] and [`PageTable`].

use crate::config::PAGE_SIZE;
use super::{frame_alloc, FrameTracker, PhysPageNum, VirtAddr, VirtPageNum};
use alloc::vec;
use alloc::vec::Vec;
use bitflags::*;

bitflags! {
    /// page table entry flags
    pub struct PTEFlags: u8 {
        /// Valid
        const V = 1 << 0;
        /// Readable
        const R = 1 << 1;
        /// Writable
        const W = 1 << 2;
        /// eXecutable
        const X = 1 << 3;
        /// User
        const U = 1 << 4;
        /// Global
        const G = 1 << 5;
        /// Accessed
        const A = 1 << 6;
        /// Dirty
        const D = 1 << 7;
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
/// page table entry structure
pub struct PageTableEntry {
    /// bits of page table entry
    pub bits: usize,
}

impl PageTableEntry {
    /// Create a new page table entry
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        PageTableEntry {
            bits: (ppn.0 << 10) | (flags.bits() as usize),
        }
    }
    /// Create an empty page table entry
    pub fn empty() -> Self {
        PageTableEntry { bits: 0 }
    }
    /// Get the physical page number from the page table entry
    pub fn ppn(&self) -> PhysPageNum {
        ((self.bits >> 10) & ((1usize << 44) - 1)).into()
    }
    /// Get the flags from the page table entry
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits(((self.bits) & 0xff) as u8).unwrap()
    }
    /// The page pointered by page table entry is valid?
    pub fn is_valid(&self) -> bool {
        (self.flags() & PTEFlags::V) != PTEFlags::empty()
    }
    /// readable?
    pub fn readable(&self) -> bool {
        (self.flags() & PTEFlags::R) != PTEFlags::empty()
    }
    /// writable?
    pub fn writable(&self) -> bool {
        (self.flags() & PTEFlags::W) != PTEFlags::empty()
    }
    /// executable?
    pub fn executable(&self) -> bool {
        (self.flags() & PTEFlags::X) != PTEFlags::empty()
    }
}
///
pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

/// Assume that it won't oom when creating/mapping.
impl PageTable {
    /// Create a new page table
    pub fn new() -> Self {
        let frame = frame_alloc().unwrap();
        PageTable {
            root_ppn: frame.ppn,
            frames: vec![frame],
        }
    }

    /// Temporarily used to get arguments from user space.
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }

    /// Find PageTableEntry by VirtPageNum, create a frame for a 4KB page table if not exist
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                *pte = PageTableEntry::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }

    /// Find PageTableEntry by VirtPageNum
    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        // We'll need a mutable reference to PTE even in translate; temporarily get pointer then transmute
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                return Some(pte);
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        None
    }

    /// set the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }

    /// remove the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }

    /// get the page table entry from the virtual page number
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        // find_pte returns Option<&mut PageTableEntry>, convert to copy
        // SAFETY: we only read the bits
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        for (i, idx) in idxs.iter().enumerate() {
            let pte_ref = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                return Some(*pte_ref);
            }
            if !pte_ref.is_valid() {
                return None;
            }
            ppn = pte_ref.ppn();
        }
        None
    }

    /// get the token from the page table
    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }
}

/// Translate&Copy a ptr[u8] array with LENGTH len to a mutable u8 Vec through page table
pub fn translated_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let vpn = start_va.floor();
        let pte = page_table.translate(vpn).unwrap();
        let ppn = pte.ppn();
        // compute page_end and chunk len explicitly
        let page_start = (vpn.0) * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let current_end = core::cmp::min(end, page_end);
        let offset = start_va.page_offset();
        let len_here = current_end - (page_start + offset);
        v.push(&mut ppn.get_bytes_array()[offset..offset + len_here]);
        start = current_end;
    }
    v
}

/// 将用户空间只读缓冲区转换为内核可访问的切片（检查读权限）
pub fn translated_slice(token: usize, ptr: usize, len: usize) -> Option<Vec<&'static [u8]>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let page_table = PageTable::from_token(token);
    let mut start = ptr;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let vpn = start_va.floor();
        let pte = page_table.translate(vpn)?;
        if !pte.is_valid() || !pte.readable() {
            return None;
        }
        let ppn = pte.ppn();
        let page_start = (vpn.0) * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let current_end = core::cmp::min(end, page_end);
        let offset = start_va.page_offset();
        let len_here = current_end - (page_start + offset);
        v.push(&ppn.get_bytes_array()[offset..offset + len_here]);
        start = current_end;
    }
    Some(v)
}

/// 将用户空间可写缓冲区转换为内核可访问的可变切片（检查写权限）
pub fn translated_mut_slice(token: usize, ptr: usize, len: usize) -> Option<Vec<&'static mut [u8]>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let page_table = PageTable::from_token(token);
    let mut start = ptr;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let vpn = start_va.floor();
        let pte = page_table.translate(vpn)?;
        if !pte.is_valid() || !pte.writable() {
            return None;
        }
        let ppn = pte.ppn();
        let page_start = (vpn.0) * PAGE_SIZE;
        let page_end = page_start + PAGE_SIZE;
        let current_end = core::cmp::min(end, page_end);
        let offset = start_va.page_offset();
        let len_here = current_end - (page_start + offset);
        v.push(&mut ppn.get_bytes_array()[offset..offset + len_here]);
        start = current_end;
    }
    Some(v)
}
