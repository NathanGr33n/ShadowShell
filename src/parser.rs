//! Tokenizes and parses a single line of shell input into a structured
//! command (program name plus arguments), honoring POSIX-style quoting
//! and escaping rules. Phase 1 only supports simple commands: no
//! pipelines, redirection, or control-flow constructs.

use std::fmt;

/// A parsed simple command: the program to run and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Errors that can occur while tokenizing a line of input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A single or double quote was opened but never closed.
    UnterminatedQuote,
    /// A backslash appeared as the final character with nothing to escape.
    TrailingBackslash,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnterminatedQuote => write!(f, "syntax error: unterminated quote"),
            ParseError::TrailingBackslash => write!(f, "syntax error: trailing backslash"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Tokenizes `line` into words, honoring POSIX-style single/double quoting
/// and backslash escaping, then returns the first word as the program and
/// the rest as arguments. Returns `Ok(None)` for blank/whitespace-only
/// input.
pub fn parse_line(line: &str) -> Result<Option<ParsedCommand>, ParseError> {
    let mut words = tokenize(line)?.into_iter();
    match words.next() {
        None => Ok(None),
        Some(program) => Ok(Some(ParsedCommand {
            program,
            args: words.collect(),
        })),
    }
}

/// Splits `line` into whitespace-separated words, treating quoted and
/// escaped sections as part of the same word rather than as separators.
fn tokenize(line: &str) -> Result<Vec<String>, ParseError> {
    #[derive(PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote = Quote::None;
    let mut chars = line.chars();

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
                ' ' | '\t' => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
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
    if in_word {
        words.push(current);
    }

    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_line_returns_none() {
        assert_eq!(parse_line("").unwrap(), None);
        assert_eq!(parse_line("   \t  ").unwrap(), None);
    }

    #[test]
    fn simple_command_with_args() {
        let cmd = parse_line("ls -la /tmp").unwrap().unwrap();
        assert_eq!(cmd.program, "ls");
        assert_eq!(cmd.args, vec!["-la", "/tmp"]);
    }

    #[test]
    fn single_quotes_preserve_literal_text() {
        let cmd = parse_line("echo 'hello  world'").unwrap().unwrap();
        assert_eq!(cmd.args, vec!["hello  world"]);
    }

    #[test]
    fn double_quotes_allow_escaped_quote() {
        let cmd = parse_line(r#"echo "say \"hi\"""#).unwrap().unwrap();
        assert_eq!(cmd.args, vec!["say \"hi\""]);
    }

    #[test]
    fn backslash_escapes_space_outside_quotes() {
        let cmd = parse_line(r"touch foo\ bar").unwrap().unwrap();
        assert_eq!(cmd.args, vec!["foo bar"]);
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
        let cmd = parse_line("echo hi\\\nthere").unwrap().unwrap();
        assert_eq!(cmd.args, vec!["hithere"]);
    }

    #[test]
    fn backslash_newline_splices_words_inside_double_quotes() {
        let cmd = parse_line("echo \"hi\\\nthere\"").unwrap().unwrap();
        assert_eq!(cmd.args, vec!["hithere"]);
    }

    #[test]
    fn single_quotes_preserve_embedded_newline() {
        let cmd = parse_line("echo 'hello\nworld'").unwrap().unwrap();
        assert_eq!(cmd.args, vec!["hello\nworld"]);
    }
}
