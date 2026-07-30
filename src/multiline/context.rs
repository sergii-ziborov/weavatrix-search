use crate::report::ContextLine;
use memchr::{memchr, memrchr};

#[derive(Clone)]
pub(super) struct LineCursor<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) number: u64,
    pub(super) start: usize,
    pub(super) full_end: usize,
}

impl<'a> LineCursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            number: 1,
            start: 0,
            full_end: next_line_end(bytes, 0),
        }
    }

    pub(super) fn advance_to(&mut self, offset: usize) {
        while self.full_end < self.bytes.len() && offset >= self.full_end {
            self.start = self.full_end;
            self.full_end = next_line_end(self.bytes, self.start);
            self.number = self.number.saturating_add(1);
        }
    }
}

fn next_line_end(bytes: &[u8], start: usize) -> usize {
    memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |relative| start + relative + 1)
}

pub(super) fn context_before(
    bytes: &[u8],
    start: usize,
    start_line: u64,
    count: usize,
    lossy: bool,
) -> Vec<ContextLine> {
    let mut context = Vec::with_capacity(count);
    let mut end = start;
    let mut line_number = start_line;
    for _ in 0..count {
        if end == 0 || line_number <= 1 {
            break;
        }
        let content_end = trim_line_end(bytes, end);
        let line_start = memrchr(b'\n', &bytes[..content_end]).map_or(0, |index| index + 1);
        line_number -= 1;
        context.push(ContextLine {
            line_number,
            text: std::str::from_utf8(&bytes[line_start..content_end])
                .expect("decoded context is UTF-8")
                .to_owned(),
            lossy,
        });
        end = line_start;
    }
    context.reverse();
    context
}

pub(super) fn context_after(
    bytes: &[u8],
    start: usize,
    end_line: u64,
    count: usize,
    lossy: bool,
) -> Vec<ContextLine> {
    let mut context = Vec::with_capacity(count);
    let mut line_start = start;
    let mut line_number = end_line;
    for _ in 0..count {
        if line_start >= bytes.len() {
            break;
        }
        let full_end = next_line_end(bytes, line_start);
        let content_end = trim_line_end(bytes, full_end);
        line_number = line_number.saturating_add(1);
        context.push(ContextLine {
            line_number,
            text: std::str::from_utf8(&bytes[line_start..content_end])
                .expect("decoded context is UTF-8")
                .to_owned(),
            lossy,
        });
        line_start = full_end;
    }
    context
}

fn trim_line_end(bytes: &[u8], mut end: usize) -> usize {
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    end
}
