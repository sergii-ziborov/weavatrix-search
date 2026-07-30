use super::{ColorChoice, OutputOptions};
use crate::options::ResultMode;
use crate::report::{ContextLine, MatchSpan, SearchMatch, SearchReport};
use std::collections::{BTreeSet, HashSet};
use std::io::{self, Write};

pub(super) fn write_text(
    report: &SearchReport,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    match report.result_mode {
        ResultMode::Matches => write_matches(report, options, destination)?,
        ResultMode::Count => {
            for file in &report.matched_files {
                let path = display_path(report, file.root_index, &file.path);
                write_path(destination, &path, options)?;
                writeln!(destination, "{}", file.matching_lines)?;
            }
        }
        ResultMode::Files => {
            for file in &report.matched_files {
                let path = display_path(report, file.root_index, &file.path);
                if options.null {
                    write!(destination, "{path}\0")?;
                } else {
                    writeln!(destination, "{path}")?;
                }
            }
        }
        ResultMode::Quiet => {}
    }
    Ok(())
}

fn write_matches(
    report: &SearchReport,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    let matching_lines = report
        .matches
        .iter()
        .flat_map(|found| {
            (found.line_number..=found.end_line_number)
                .map(move |line| (found.root_index, found.path.as_str(), line))
        })
        .collect::<HashSet<_>>();
    let mut emitted_context = BTreeSet::new();
    let mut current_heading = None;
    for found in &report.matches {
        let path = display_path(report, found.root_index, &found.path);
        if options.heading && current_heading.as_deref() != Some(path.as_str()) {
            if current_heading.is_some() {
                writeln!(destination)?;
            }
            write_heading(destination, &path, options)?;
            current_heading = Some(path.clone());
        }
        write_context_lines(
            found,
            &found.before,
            &path,
            &matching_lines,
            &mut emitted_context,
            options,
            destination,
        )?;
        write_found(found, &path, options, destination)?;
        write_context_lines(
            found,
            &found.after,
            &path,
            &matching_lines,
            &mut emitted_context,
            options,
            destination,
        )?;
    }
    Ok(())
}

fn write_context_lines(
    found: &SearchMatch,
    lines: &[ContextLine],
    path: &str,
    matching_lines: &HashSet<(usize, &str, u64)>,
    emitted: &mut BTreeSet<(usize, String, u64)>,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    for context in lines {
        let key = (found.root_index, found.path.as_str(), context.line_number);
        if !matching_lines.contains(&key)
            && emitted.insert((found.root_index, found.path.clone(), context.line_number))
        {
            write_text_line(
                destination,
                TextRecord {
                    path,
                    line_number: context.line_number,
                    column: None,
                    separator: '-',
                    text: &context.text,
                    spans: &[],
                },
                options,
            )?;
        }
    }
    Ok(())
}

fn write_found(
    found: &SearchMatch,
    path: &str,
    options: OutputOptions,
    destination: &mut impl Write,
) -> io::Result<()> {
    if options.only_matching {
        for span in &found.spans {
            let text = found.line.get(span.start..span.end).unwrap_or_default();
            let (line_number, column) = line_and_column(&found.line, found.line_number, span.start);
            write_text_line(
                destination,
                TextRecord {
                    path,
                    line_number,
                    column: Some(column),
                    separator: ':',
                    text,
                    spans: &[],
                },
                options,
            )?;
        }
        return Ok(());
    }

    let preview = found.replacement_preview.as_deref();
    let column = found
        .spans
        .first()
        .map(|span| line_and_column(&found.line, found.line_number, span.start).1);
    write_text_line(
        destination,
        TextRecord {
            path,
            line_number: found.line_number,
            column,
            separator: ':',
            text: preview.unwrap_or(&found.line),
            spans: if preview.is_some() { &[] } else { &found.spans },
        },
        options,
    )
}

#[derive(Clone, Copy)]
struct TextRecord<'a> {
    path: &'a str,
    line_number: u64,
    column: Option<usize>,
    separator: char,
    text: &'a str,
    spans: &'a [MatchSpan],
}

fn write_text_line(
    destination: &mut impl Write,
    record: TextRecord<'_>,
    options: OutputOptions,
) -> io::Result<()> {
    if !options.heading {
        write_colored(destination, record.path, "\x1b[35m", options.color)?;
        if options.null {
            write!(destination, "\0")?;
        } else {
            write!(destination, "{}", record.separator)?;
        }
    }
    if options.line_number {
        write_colored(
            destination,
            &record.line_number.to_string(),
            "\x1b[32m",
            options.color,
        )?;
        write!(destination, "{}", record.separator)?;
    }
    if options.column && record.separator == ':' {
        write_colored(
            destination,
            &record.column.unwrap_or(1).to_string(),
            "\x1b[32m",
            options.color,
        )?;
        write!(destination, "{}", record.separator)?;
    }
    write_highlighted(destination, record.text, record.spans, options.color)?;
    if !record.text.ends_with('\n') {
        writeln!(destination)?;
    }
    Ok(())
}

fn display_path(report: &SearchReport, root_index: usize, relative: &str) -> String {
    if report.roots.len() <= 1 {
        return relative.to_owned();
    }
    report
        .roots
        .get(root_index)
        .map_or_else(
            || format!("{root_index}/{relative}"),
            |root| root.join(relative).to_string_lossy().into_owned(),
        )
        .replace('\\', "/")
}

fn line_and_column(text: &str, first_line: u64, offset: usize) -> (u64, usize) {
    let prefix = text.get(..offset).unwrap_or_default();
    let extra_lines =
        u64::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).unwrap_or(u64::MAX);
    let line_start = prefix
        .rfind('\n')
        .map_or(0, |index| index.saturating_add(1));
    (
        first_line.saturating_add(extra_lines),
        offset.saturating_sub(line_start).saturating_add(1),
    )
}

fn write_heading(
    destination: &mut impl Write,
    path: &str,
    options: OutputOptions,
) -> io::Result<()> {
    write_colored(destination, path, "\x1b[35m", options.color)?;
    if options.null {
        write!(destination, "\0")
    } else {
        writeln!(destination)
    }
}

fn write_path(destination: &mut impl Write, path: &str, options: OutputOptions) -> io::Result<()> {
    write_colored(destination, path, "\x1b[35m", options.color)?;
    if options.null {
        write!(destination, "\0")
    } else {
        write!(destination, ":")
    }
}

fn write_colored(
    destination: &mut impl Write,
    text: &str,
    color: &str,
    choice: ColorChoice,
) -> io::Result<()> {
    if choice == ColorChoice::Always {
        write!(destination, "{color}{text}\x1b[0m")
    } else {
        write!(destination, "{text}")
    }
}

fn write_highlighted(
    destination: &mut impl Write,
    text: &str,
    spans: &[MatchSpan],
    choice: ColorChoice,
) -> io::Result<()> {
    if choice == ColorChoice::Never || spans.is_empty() {
        return write!(destination, "{text}");
    }
    let mut cursor = 0;
    for span in spans {
        let Some(prefix) = text.get(cursor..span.start) else {
            return write!(destination, "{text}");
        };
        let Some(matched) = text.get(span.start..span.end) else {
            return write!(destination, "{text}");
        };
        write!(destination, "{prefix}\x1b[1;31m{matched}\x1b[0m")?;
        cursor = span.end;
    }
    write!(destination, "{}", text.get(cursor..).unwrap_or_default())
}
