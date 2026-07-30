use super::{
    Arguments, CaseMode, CliColor, EncodingMode, ImmediateAction, IndexOptions, OsString,
    OutputFormat, PathBuf, QuerySyntax, ResultMode, SearchMode, SearchOptions, SearchQuery,
    next_number, next_path, next_string, parse_color, parse_discovery, parse_encoding,
    parse_number,
};

impl Arguments {
    pub(super) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let defaults = SearchOptions::default();
        let mut parsed = Self {
            patterns: Vec::new(),
            positional: Vec::new(),
            syntax: QuerySyntax::Regex,
            case: defaults.case,
            before_context: 0,
            after_context: 0,
            mode: SearchMode::Line,
            result_mode: ResultMode::Matches,
            encoding: EncodingMode::Auto,
            replacement: None,
            globs: Vec::new(),
            hidden: false,
            archives: defaults.archives.enabled,
            format: OutputFormat::Text,
            heading: false,
            line_number: true,
            column: false,
            only_matching: false,
            null: false,
            color: CliColor::Auto,
            stats: false,
            max_results: defaults.max_results,
            max_file_bytes: defaults.max_file_bytes,
            max_line_bytes: defaults.max_line_bytes,
            max_multiline_bytes: defaults.max_multiline_bytes,
            max_replacement_bytes: defaults.max_replacement_bytes,
            max_decoder_memory_bytes: defaults.archives.max_decoder_memory_bytes,
            discovery: None,
            content_workers: None,
            index: None,
            rebuild_index: false,
            index_workers: IndexOptions::default().search_parallelism,
            index_status: None,
            roots_explicit: false,
            action: None,
            roots: Vec::new(),
        };
        let mut arguments = arguments.into_iter();
        let mut options = true;
        while let Some(argument) = arguments.next() {
            if options && argument == "--" {
                options = false;
                continue;
            }
            let is_option = options
                && argument
                    .to_str()
                    .is_some_and(|value| value.starts_with('-') && value != "-");
            if !is_option {
                parsed.positional.push(argument);
                continue;
            }
            let argument = argument
                .into_string()
                .expect("option-like arguments were verified as UTF-8");
            parsed.parse_option(&argument, &mut arguments)?;
        }
        parsed.finish()
    }

    fn parse_option(
        &mut self,
        argument: &str,
        arguments: &mut impl Iterator<Item = OsString>,
    ) -> Result<(), String> {
        match argument {
            "-h" | "--help" => self.action = Some(ImmediateAction::Help),
            "-V" | "--version" => self.action = Some(ImmediateAction::Version),
            "-F" | "--fixed-strings" => self.syntax = QuerySyntax::Literal,
            "-i" | "--ignore-case" => self.case = CaseMode::Insensitive,
            "-s" | "--case-sensitive" => self.case = CaseMode::Sensitive,
            "-S" | "--smart-case" => self.case = CaseMode::Smart,
            "-U" | "--multiline" => self.mode = SearchMode::Multiline,
            "-c" | "--count" => self.set_result_mode(ResultMode::Count)?,
            "-l" | "--files-with-matches" => self.set_result_mode(ResultMode::Files)?,
            "-q" | "--quiet" => self.set_result_mode(ResultMode::Quiet)?,
            "--json" => self.format = OutputFormat::JsonLines,
            "--heading" => self.heading = true,
            "--no-heading" => self.heading = false,
            "-n" | "--line-number" => self.line_number = true,
            "-N" | "--no-line-number" => self.line_number = false,
            "--column" => {
                self.column = true;
                self.line_number = true;
            }
            "-o" | "--only-matching" => self.only_matching = true,
            "-0" | "--null" => self.null = true,
            "--stats" => self.stats = true,
            "--index" => self.index = Some(next_path(arguments, argument)?),
            "--rebuild-index" => self.rebuild_index = true,
            "--index-workers" => self.index_workers = next_number(arguments, argument)?,
            "--index-status" => self.index_status = Some(next_path(arguments, argument)?),
            "--hidden" => self.hidden = true,
            "--no-archives" => self.archives = false,
            "--discovery" => {
                self.discovery = parse_discovery(&next_string(arguments, argument)?)?;
            }
            "--content-workers" => {
                self.content_workers = Some(next_number(arguments, argument)?);
            }
            "-e" | "--regexp" => self.patterns.push(next_string(arguments, argument)?),
            "-r" | "--replace" => self.replacement = Some(next_string(arguments, argument)?),
            "-g" | "--glob" => self.globs.push(next_string(arguments, argument)?),
            "-A" | "--after-context" => {
                self.after_context = next_number(arguments, argument)?;
            }
            "-B" | "--before-context" => {
                self.before_context = next_number(arguments, argument)?;
            }
            "-C" | "--context" => {
                let context = next_number(arguments, argument)?;
                self.before_context = context;
                self.after_context = context;
            }
            "--encoding" => {
                self.encoding = parse_encoding(&next_string(arguments, argument)?)?;
            }
            "--color" => {
                self.color = parse_color(&next_string(arguments, argument)?)?;
            }
            "--max-results" => self.max_results = next_number(arguments, argument)?,
            "--max-file-bytes" => self.max_file_bytes = next_number(arguments, argument)?,
            "--max-line-bytes" => self.max_line_bytes = next_number(arguments, argument)?,
            "--max-multiline-bytes" => {
                self.max_multiline_bytes = next_number(arguments, argument)?;
            }
            "--max-replacement-bytes" => {
                self.max_replacement_bytes = next_number(arguments, argument)?;
            }
            "--max-decoder-memory-bytes" => {
                self.max_decoder_memory_bytes = next_number(arguments, argument)?;
            }
            _ => {
                if let Some((name, value)) = argument.split_once('=') {
                    self.parse_assignment(name, value)?;
                } else {
                    return Err(format!("unknown option {argument}; use --help"));
                }
            }
        }
        Ok(())
    }

    fn parse_assignment(&mut self, name: &str, value: &str) -> Result<(), String> {
        match name {
            "--regexp" => self.patterns.push(value.to_owned()),
            "--replace" => self.replacement = Some(value.to_owned()),
            "--glob" => self.globs.push(value.to_owned()),
            "--encoding" => self.encoding = parse_encoding(value)?,
            "--color" => self.color = parse_color(value)?,
            "--after-context" => self.after_context = parse_number(name, value)?,
            "--before-context" => self.before_context = parse_number(name, value)?,
            "--context" => {
                let context = parse_number(name, value)?;
                self.before_context = context;
                self.after_context = context;
            }
            "--max-results" => self.max_results = parse_number(name, value)?,
            "--max-file-bytes" => self.max_file_bytes = parse_number(name, value)?,
            "--max-line-bytes" => self.max_line_bytes = parse_number(name, value)?,
            "--max-multiline-bytes" => self.max_multiline_bytes = parse_number(name, value)?,
            "--max-replacement-bytes" => {
                self.max_replacement_bytes = parse_number(name, value)?;
            }
            "--max-decoder-memory-bytes" => {
                self.max_decoder_memory_bytes = parse_number(name, value)?;
            }
            "--discovery" => self.discovery = parse_discovery(value)?,
            "--content-workers" => self.content_workers = Some(parse_number(name, value)?),
            "--index" => self.index = Some(PathBuf::from(value)),
            "--index-workers" => self.index_workers = parse_number(name, value)?,
            "--index-status" => self.index_status = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {name}; use --help")),
        }
        Ok(())
    }

    fn set_result_mode(&mut self, mode: ResultMode) -> Result<(), String> {
        if self.result_mode != ResultMode::Matches && self.result_mode != mode {
            return Err(
                "only one of --count, --files-with-matches, or --quiet is allowed".to_owned(),
            );
        }
        self.result_mode = mode;
        Ok(())
    }

    fn finish(mut self) -> Result<Self, String> {
        if self.action.is_some() || self.index_status.is_some() {
            return Ok(self);
        }
        if self.patterns.is_empty() {
            if self.positional.is_empty() {
                return Err("a PATTERN is required; use --help".to_owned());
            }
            self.patterns.push(
                self.positional
                    .remove(0)
                    .into_string()
                    .map_err(|_| "PATTERN must be valid UTF-8".to_owned())?,
            );
        }
        if self.positional.is_empty() {
            self.roots.push(PathBuf::from("."));
        } else {
            self.roots_explicit = true;
            self.roots
                .extend(self.positional.drain(..).map(PathBuf::from));
        }
        if self.rebuild_index && self.index.is_none() {
            return Err("--rebuild-index requires --index PATH".to_owned());
        }
        if self.replacement.is_some() && self.result_mode != ResultMode::Matches {
            return Err("--replace requires match output mode".to_owned());
        }
        if self.replacement.is_some() && self.only_matching {
            return Err("--replace and --only-matching cannot be combined".to_owned());
        }
        if self.only_matching && (self.before_context > 0 || self.after_context > 0) {
            return Err("--only-matching cannot be combined with context".to_owned());
        }
        if self.content_workers == Some(0) {
            return Err("--content-workers must be greater than zero".to_owned());
        }
        Ok(self)
    }

    pub(super) fn query(&self) -> Result<SearchQuery, String> {
        if self.patterns.iter().any(String::is_empty) {
            return Err("search patterns must not be empty".to_owned());
        }
        let mut queries = self.patterns.iter().map(|pattern| {
            if self.syntax == QuerySyntax::Literal {
                SearchQuery::literal(pattern)
            } else {
                SearchQuery::regex(pattern)
            }
        });
        let first = queries
            .next()
            .ok_or_else(|| "a PATTERN is required".to_owned())?;
        Ok(if self.patterns.len() == 1 {
            first
        } else {
            SearchQuery::any(std::iter::once(first).chain(queries))
        })
    }
}
