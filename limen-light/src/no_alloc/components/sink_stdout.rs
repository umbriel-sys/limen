#![cfg(feature = "std")]
use limen_core::errors::OutputError;
use crate::no_alloc::traits::OutputSinkNoAlloc;
use crate::no_alloc::types::BorrowedTensorView;

pub struct StandardOutputSinkNoAlloc { preview_bytes: usize }
impl StandardOutputSinkNoAlloc { pub fn new(preview_bytes: usize) -> Self { Self { preview_bytes } } }
impl OutputSinkNoAlloc for StandardOutputSinkNoAlloc {
    fn write(&mut self, output: BorrowedTensorView<'_>) -> Result<(), OutputError> {
        let preview_len = core::cmp::min(self.preview_bytes, output.buffer.len());
        let mut hex = String::new();
        for b in &output.buffer[..preview_len] {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{:02X} ", b);
        }
        println!("NoAllocSink: data_type={}, shape={:?}, bytes={}, preview=[{}]", output.data_type, output.shape, output.buffer.len(), hex.trim_end());
        Ok(())
    }
}
