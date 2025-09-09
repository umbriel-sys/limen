use limen_core::errors::OutputError;
use limen_core::traits::{OutputSink, OutputSinkFactory};
use limen_core::traits::configuration::OutputSinkConfiguration;
use limen_core::types::TensorOutput;

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

pub struct FileOutputSink {
    writer: BufWriter<std::fs::File>,
    show_preview_bytes: usize,
}

impl FileOutputSink {
    fn write_line(&mut self, line: &str) -> Result<(), OutputError> {
        self.writer.write_all(line.as_bytes()).and_then(|_| self.writer.write_all(b"\n")).map_err(|_| OutputError::WriteFailed)
    }
}

impl OutputSink for FileOutputSink {
    fn write(&mut self, output: &TensorOutput) -> Result<(), OutputError> {
        let preview_len = self.show_preview_bytes.min(output.buffer.len());
        let mut preview_hex = String::new();
        for byte in output.buffer.iter().take(preview_len) {
            use std::fmt::Write as _;
            let _ = write!(&mut preview_hex, "{:02X} ", byte);
        }
        let line = format!("data_type={}, shape={:?}, bytes={}, preview=[{}]", output.data_type, output.shape, output.buffer.len(), preview_hex.trim_end());
        self.write_line(&line)
    }
    fn flush(&mut self) -> Result<(), OutputError> { self.writer.flush().map_err(|_| OutputError::FlushFailed) }
    fn close(&mut self) -> Result<(), OutputError> { self.flush() }
}

pub struct FileOutputSinkFactory;
impl OutputSinkFactory for FileOutputSinkFactory {
    fn sink_name(&self) -> &'static str { "file" }
    fn create_output_sink(&self, configuration: &OutputSinkConfiguration) -> Result<Box<dyn OutputSink>, OutputError> {
        let path = configuration.parameters.get("path").ok_or(OutputError::Other { message: "missing 'path'".to_string() })?;
        let append = configuration.parameters.get("append").map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(true);
        let show_preview_bytes = configuration.parameters.get("preview_bytes").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0usize);
        let file = OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(path).map_err(|_| OutputError::WriteFailed)?;
        Ok(Box::new(FileOutputSink { writer: BufWriter::new(file), show_preview_bytes }))
    }
}
