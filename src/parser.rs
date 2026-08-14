//! Tokenizes and parses a single line of shell input into a [`Pipeline`]
//! of one or more [`SimpleCommand`]s, honoring POSIX-style quoting and
//! escaping, pipelines (`|`), redirection (`<`, `>`, `>>`, `2>`, `2>>`),
//! and background execution (`&`). Control-flow constructs and further
//! scripting syntax are out of scope until Phase 5.

use std::fmt;

/// A single command within a pipeline: the program to run, its
/// arguments, and any redirections attached to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub program: String,
    pub args: Vec<String>,
    pub redirects: Vec<Redirect>,
}

/// A single I/O redirection attached to a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub kind: RedirectKind,
    pub target: String,
}

/// The direction and mode of a redirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `<file`: read the command's stdin from `file`.
    Stdin,
    /// `>file`: write the command's stdout to `file`, truncating it.
    StdoutTruncate,
    /// `>>file`: write the command's stdout to `file`, appending to it.
    StdoutAppend,
    /// `2>file`: write the command's stderr to `file`, truncating it.
    StderrTruncate,
    /// `2>>file`: write the command's stderr to `file`, appending to it.
    StderrAppend,
}

/// One or more simple commands connected by pipes, optionally run in the
/// background instead of being waited on immediately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
    pub background: bool,
}

/// Errors that can occur while tokenizing or structuring a line of input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A single or double quote was opened but never closed.
    UnterminatedQuote,
    /// A backslash appeared as the final character with nothing to escape.
    TrailingBackslash,
    /// A pipeline segment was empty, e.g. a leading, trailing, or doubled `|`.
    EmptyPipelineSegment,
    /// A redirection operator was not followed by a target word.
    MissingRedirectTarget,
    /// `&` appeared somewhere other than as the final token of the line.
    MisplacedBackground,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnterminatedQuote => write!(f, "syntax error: unterminated quote"),
            ParseError::TrailingBackslash => write!(f, "syntax error: trailing backslash"),
            ParseError::EmptyPipelineSegment => write!(f, "syntax error: empty command near `|'"),
            ParseError::MissingRedirectTarget => {
                write!(f, "syntax error: expected a file name after redirection operator")
            }
            ParseError::MisplacedBackground => {
                write!(f, "syntax error: `&' must appear only at the end of the line")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// A lexical token produced by [`tokenize`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    Background,
    Less,
    Great,
    DGreat,
    ErrGreat,
    ErrDGreat,
}

/// Tokenizes and structures `line` into a [`Pipeline`]. Returns `Ok(None)`
/// for blank/whitespace-only input.
pub fn parse_line(line: &str) -> Result<Option<Pipeline>, ParseError> {
    build_pipeline(tokenize(line)?)
}

/// Turns a flat token stream into a [`Pipeline`], splitting on
/// [`Token::Pipe`] and validating that a trailing `&` (if present) is the
/// only one and appears at the very end.
fn build_pipeline(mut tokens: Vec<Token>) -> Result<Option<Pipeline>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let background = matches!(tokens.last(), Some(Token::Background));
    if background {
        tokens.pop();
    }
    // Any `&` remaining at this point is either doubled up (`&&`, parsed as
    // two Background tokens) or appeared before the end of the line, both
    // of which are unsupported command-separator syntax in this phase.
    if tokens.contains(&Token::Background) {
        return Err(ParseError::MisplacedBackground);
    }

    let mut commands = Vec::new();
    for segment in tokens.split(|t| *t == Token::Pipe) {
        commands.push(build_simple_command(segment)?);
    }

    Ok(Some(Pipeline { commands, background }))
}

/// Builds a single [`SimpleCommand`] from one pipeline segment's tokens.
/// Redirections may appear anywhere in the segment (before, after, or
/// between argument words), matching shell convention.
fn build_simple_command(segment: &[Token]) -> Result<SimpleCommand, ParseError> {
    let mut words = Vec::new();
    let mut redirects = Vec::new();
    let mut iter = segment.iter();

    while let Some(token) = iter.next() {
        let kind = match token {
            Token::Word(word) => {
                words.push(word.clone());
                continue;
            }
            Token::Less => RedirectKind::Stdin,
            Token::Great => RedirectKind::StdoutTruncate,
            Token::DGreat => RedirectKind::StdoutAppend,
            Token::ErrGreat => RedirectKind::StderrTruncate,
            Token::ErrDGreat => RedirectKind::StderrAppend,
            Token::Pipe | Token::Background => {
                unreachable!("Pipe splits segments and Background is stripped before this point")
            }
        };
        match iter.next() {
            Some(Token::Word(target)) => redirects.push(Redirect {
                kind,
                target: target.clone(),
            }),
            _ => return Err(ParseError::MissingRedirectTarget),
        }
    }

    let mut words = words.into_iter();
    let program = words.next().ok_or(ParseError::EmptyPipelineSegment)?;
    Ok(SimpleCommand {
        program,
        args: words.collect(),
        redirects,
    })
}

/// Flushes the word currently being accumulated (if any) as a [`Token::Word`].
fn flush_word(tokens: &mut Vec<Token>, current: &mut String, in_word: &mut bool) {
    if *in_word {
        tokens.push(Token::Word(std::mem::take(current)));
        *in_word = false;
    }
}

/// Splits `line` into a token stream: words (honoring POSIX-style
/// single/double quoting and backslash escaping) plus the pipeline,
/// redirection, and background operators. A bare `2` immediately
/// followed by `>`/`>>` (no intervening whitespace) is treated as the
/// file-descriptor prefix of a stderr redirect rather than a word,
/// matching POSIX `IO_NUMBER` handling for that specific case.
fn tokenize(line: &str) -> Result<Vec<Token>, ParseError> {
    #[derive(PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote = Quote::None;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                // Single quotes preserve everything literally; no escapes.
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(c);
                }
            }
            Quote::Double => {
                if c == '"' {
                    quote = Quote::None;
                } else if c == '\\' {
                    // Inside double quotes, backslash only escapes
                    // characters that are otherwise special there.
                    match chars.next() {
                        // A backslash-newline is a POSIX line continuation:
                        // both characters are removed and the following
                        // text is spliced directly onto the current word.
                        Some('\n') => {}
                        Some(next @ ('"' | '\\' | '$' | '`')) => current.push(next),
                        Some(other) => {
                            current.push('\\');
                            current.push(other);
                        }
                        None => return Err(ParseError::UnterminatedQuote),
                    }
                } else {
                    current.push(c);
                }
            }
            Quote::None => match c {
                ' ' | '\t' => flush_word(&mut tokens, &mut current, &mut in_word),
                '\'' => {
                    quote = Quote::Single;
                    in_word = true;
                }
                '"' => {
                    quote = Quote::Double;
                    in_word = true;
                }
                '\\' => {
                    // Outside quotes, backslash escapes the next character
                    // literally, including whitespace. A backslash-newline
                    // is instead a POSIX line continuation: both characters
                    // are removed with nothing spliced in their place, which
                    // lets multiline input (from the line editor's validator)
                    // continue a word across physical lines.
                    in_word = true;
                    match chars.next() {
                        Some('\n') => {}
                        Some(next) => current.push(next),
                        None => return Err(ParseError::TrailingBackslash),
                    }
                }
                '|' => {
                    flush_word(&mut tokens, &mut current, &mut in_word);
                    tokens.push(Token::Pipe);
                }
                '&' => {
                    flush_word(&mut tokens, &mut current, &mut in_word);
                    tokens.push(Token::Background);
                }
                '<' => {
                    flush_word(&mut tokens, &mut current, &mut in_word);
                    tokens.push(Token::Less);
                }
                '>' => {
                    let is_stderr = in_word && current == "2";
                    if is_stderr {
                        // The "2" was a file-descriptor prefix, not a word.
                        current.clear();
                        in_word = false;
                    } else {
                        flush_word(&mut tokens, &mut current, &mut in_word);
                    }
                    let doubled = chars.peek() == Some(&'>');
                    if doubled {
                        chars.next();
                    }
                    tokens.push(match (is_stderr, doubled) {
                        (true, true) => Token::ErrDGreat,
                        (true, false) => Token::ErrGreat,
                        (false, true) => Token::DGreat,
                        (false, false) => Token::Great,
                    });
                }
                other => {
                    in_word = true;
                    current.push(other);
                }
            },
        }
    }

    if quote != Quote::None {
        return Err(ParseError::UnterminatedQuote);
    }
    flush_word(&mut tokens, &mut current, &mut in_word);

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `line` and returns the single command's `(program, args)`,
    /// asserting there is exactly one command with no redirects and the
    /// pipeline is not backgrounded. Convenience for tests that only care
    /// about simple-command tokenizing (quoting/escaping), unaffected by
    /// the Phase 3 pipeline/redirect/background additions.
    fn parse_single(line: &str) -> (String, Vec<String>) {
        let pipeline = parse_line(line).unwrap().unwrap();
        assert_eq!(pipeline.commands.len(), 1);
        assert!(!pipeline.background);
        let cmd = &pipeline.commands[0];
        assert!(cmd.redirects.is_empty());
        (cmd.program.clone(), cmd.args.clone())
    }

    #[test]
    fn empty_line_returns_none() {
        assert_eq!(parse_line("").unwrap(), None);
        assert_eq!(parse_line("   \t  ").unwrap(), None);
    }

    #[test]
    fn simple_command_with_args() {
        let (program, args) = parse_single("ls -la /tmp");
        assert_eq!(program, "ls");
        assert_eq!(args, vec!["-la", "/tmp"]);
    }

    #[test]
    fn single_quotes_preserve_literal_text() {
        let (_, args) = parse_single("echo 'hello  world'");
        assert_eq!(args, vec!["hello  world"]);
    }

    #[test]
    fn double_quotes_allow_escaped_quote() {
        let (_, args) = parse_single(r#"echo "say \"hi\"""#);
        assert_eq!(args, vec!["say \"hi\""]);
    }

    #[test]
    fn backslash_escapes_space_outside_quotes() {
        let (_, args) = parse_single(r"touch foo\ bar");
        assert_eq!(args, vec!["foo bar"]);
    }

    #[test]
    fn unterminated_single_quote_is_error() {
        assert_eq!(
            parse_line("echo 'unterminated"),
            Err(ParseError::UnterminatedQuote)
        );
    }

    #[test]
    fn trailing_backslash_is_error() {
        assert_eq!(parse_line(r"echo \"), Err(ParseError::TrailingBackslash));
    }

    #[test]
    fn backslash_newline_splices_words_outside_quotes() {
        let (_, args) = parse_single("echo hi\\\nthere");
        assert_eq!(args, vec!["hithere"]);
    }

    #[test]
    fn backslash_newline_splices_words_inside_double_quotes() {
        let (_, args) = parse_single("echo \"hi\\\nthere\"");
        assert_eq!(args, vec!["hithere"]);
    }

    #[test]
    fn single_quotes_preserve_embedded_newline() {
        let (_, args) = parse_single("echo 'hello\nworld'");
        assert_eq!(args, vec!["hello\nworld"]);
    }

    #[test]
    fn two_stage_pipeline() {
        let pipeline = parse_line("ls | wc -l").unwrap().unwrap();
        assert_eq!(pipeline.commands.len(), 2);
        assert_eq!(pipeline.commands[0].program, "ls");
        assert!(pipeline.commands[0].args.is_empty());
        assert_eq!(pipeline.commands[1].program, "wc");
        assert_eq!(pipeline.commands[1].args, vec!["-l"]);
        assert!(!pipeline.background);
    }

    #[test]
    fn three_stage_pipeline() {
        let pipeline = parse_line("cat file | grep foo | sort").unwrap().unwrap();
        assert_eq!(pipeline.commands.len(), 3);
        assert_eq!(pipeline.commands[2].program, "sort");
    }

    #[test]
    fn leading_pipe_is_error() {
        assert_eq!(parse_line("| ls"), Err(ParseError::EmptyPipelineSegment));
    }

    #[test]
    fn trailing_pipe_is_error() {
        assert_eq!(parse_line("ls |"), Err(ParseError::EmptyPipelineSegment));
    }

    #[test]
    fn doubled_pipe_is_error() {
        assert_eq!(parse_line("ls || wc"), Err(ParseError::EmptyPipelineSegment));
    }

    #[test]
    fn stdout_truncate_redirect() {
        let pipeline = parse_line("echo hi > out.txt").unwrap().unwrap();
        let cmd = &pipeline.commands[0];
        assert_eq!(cmd.program, "echo");
        assert_eq!(cmd.args, vec!["hi"]);
        assert_eq!(
            cmd.redirects,
            vec![Redirect {
                kind: RedirectKind::StdoutTruncate,
                target: "out.txt".to_string()
            }]
        );
    }

    #[test]
    fn stdout_append_redirect() {
        let pipeline = parse_line("echo hi >> out.txt").unwrap().unwrap();
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::StdoutAppend,
                target: "out.txt".to_string()
            }]
        );
    }

    #[test]
    fn stdin_redirect() {
        let pipeline = parse_line("sort < in.txt").unwrap().unwrap();
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::Stdin,
                target: "in.txt".to_string()
            }]
        );
    }

    #[test]
    fn stderr_truncate_redirect() {
        let pipeline = parse_line("cmd 2> err.txt").unwrap().unwrap();
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::StderrTruncate,
                target: "err.txt".to_string()
            }]
        );
    }

    #[test]
    fn stderr_append_redirect() {
        let pipeline = parse_line("cmd 2>> err.txt").unwrap().unwrap();
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::StderrAppend,
                target: "err.txt".to_string()
            }]
        );
    }

    #[test]
    fn redirect_operator_without_space_is_recognized() {
        let pipeline = parse_line("echo hi>out.txt").unwrap().unwrap();
        assert_eq!(pipeline.commands[0].args, vec!["hi"]);
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::StdoutTruncate,
                target: "out.txt".to_string()
            }]
        );
    }

    #[test]
    fn digit_two_with_space_before_redirect_is_a_plain_argument() {
        let pipeline = parse_line("echo 2 > out.txt").unwrap().unwrap();
        assert_eq!(pipeline.commands[0].args, vec!["2"]);
        assert_eq!(
            pipeline.commands[0].redirects,
            vec![Redirect {
                kind: RedirectKind::StdoutTruncate,
                target: "out.txt".to_string()
            }]
        );
    }

    #[test]
    fn missing_redirect_target_is_error() {
        assert_eq!(
            parse_line("echo hi >"),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn background_flag_is_set() {
        let pipeline = parse_line("sleep 5 &").unwrap().unwrap();
        assert!(pipeline.background);
        assert_eq!(pipeline.commands[0].program, "sleep");
    }

    #[test]
    fn background_pipeline() {
        let pipeline = parse_line("sleep 5 | cat &").unwrap().unwrap();
        assert!(pipeline.background);
        assert_eq!(pipeline.commands.len(), 2);
    }

    #[test]
    fn misplaced_background_is_error() {
        assert_eq!(
            parse_line("sleep 5 & echo done"),
            Err(ParseError::MisplacedBackground)
        );
    }

    #[test]
    fn doubled_background_is_error() {
        assert_eq!(
            parse_line("cmd1 && cmd2"),
            Err(ParseError::MisplacedBackground)
        );
    }

    #[test]
    fn pipeline_with_redirect_on_last_command() {
        let pipeline = parse_line("ls | sort > out.txt").unwrap().unwrap();
        assert_eq!(pipeline.commands.len(), 2);
        assert!(pipeline.commands[0].redirects.is_empty());
        assert_eq!(
            pipeline.commands[1].redirects,
            vec![Redirect {
                kind: RedirectKind::StdoutTruncate,
                target: "out.txt".to_string()
            }]
        );
    }
}
