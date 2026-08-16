//! Tokenizes and parses shell input into a [`Program`] AST (lists, `&&`/`||`,
//! pipelines, redirections, background jobs, assignments, functions, and
//! compound commands). Words retain quote structure for later expansion.
//!
//! [`parse_line`] remains as a compatibility helper that accepts a single
//! simple pipeline and lowers it to the Phase 3 stringly-typed [`Pipeline`]
//! used by job control.

use std::fmt;

// ===========================================================================
// Legacy runtime types (Phase 3 executor / job_control)
// ===========================================================================

/// One or more simple commands connected by pipes, optionally backgrounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
    pub background: bool,
}

/// A single command within a legacy pipeline: program, args, redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub program: String,
    pub args: Vec<String>,
    pub redirects: Vec<Redirect>,
}

/// A single I/O redirection with a literal path target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub kind: RedirectKind,
    pub target: String,
}

/// The direction and mode of a redirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    Stdin,
    StdoutTruncate,
    StdoutAppend,
    StderrTruncate,
    StderrAppend,
}

// ===========================================================================
// Program AST (Phase 5)
// ===========================================================================

/// A complete script or interactive submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub lists: Vec<AndOrList>,
}

/// Pipelines joined by `&&` / `||` (left-associative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndOrList {
    pub first: AstPipeline,
    pub rest: Vec<(AndOrOp, AstPipeline)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And,
    Or,
}

/// Pipeline of AST commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstPipeline {
    pub commands: Vec<Command>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Simple(AstSimpleCommand),
    Compound(CompoundCommand),
    FunctionDef {
        name: String,
        body: Box<CompoundCommand>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstSimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirects: Vec<AstRedirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstRedirect {
    pub kind: RedirectKind,
    pub target: Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompoundCommand {
    BraceGroup(Program),
    Subshell(Program),
    If {
        condition: Program,
        then_branch: Program,
        elif_branches: Vec<(Program, Program)>,
        else_branch: Option<Program>,
    },
    While {
        condition: Program,
        body: Program,
    },
    Until {
        condition: Program,
        body: Program,
    },
    For {
        name: String,
        words: Vec<Word>,
        body: Program,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Program,
}

/// A shell word made of quote-aware pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<WordPart>,
}

impl Word {
    pub fn literal(s: impl Into<String>) -> Self {
        Word {
            parts: vec![WordPart::Unquoted(s.into())],
        }
    }

    pub fn display_raw(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                WordPart::Unquoted(s) | WordPart::SingleQuoted(s) | WordPart::DoubleQuoted(s) => {
                    out.push_str(s);
                }
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
            || self.parts.iter().all(|p| match p {
                WordPart::Unquoted(s) | WordPart::SingleQuoted(s) | WordPart::DoubleQuoted(s) => {
                    s.is_empty()
                }
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    Unquoted(String),
    SingleQuoted(String),
    DoubleQuoted(String),
}

// ===========================================================================
// Errors
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Incomplete,
    UnterminatedQuote,
    TrailingBackslash,
    EmptyPipelineSegment,
    MissingRedirectTarget,
    MisplacedBackground,
    Unexpected(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Incomplete => write!(f, "syntax error: incomplete input"),
            ParseError::UnterminatedQuote => write!(f, "syntax error: unterminated quote"),
            ParseError::TrailingBackslash => write!(f, "syntax error: trailing backslash"),
            ParseError::EmptyPipelineSegment => write!(f, "syntax error: empty command near `|'"),
            ParseError::MissingRedirectTarget => {
                write!(f, "syntax error: expected a file name after redirection operator")
            }
            ParseError::MisplacedBackground => {
                write!(f, "syntax error: `&' must appear only at the end of a pipeline")
            }
            ParseError::Unexpected(msg) => write!(f, "syntax error: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    pub fn is_incomplete(&self) -> bool {
        matches!(
            self,
            ParseError::Incomplete | ParseError::UnterminatedQuote | ParseError::TrailingBackslash
        )
    }
}

// ===========================================================================
// Tokens
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(Word),
    Newline,
    Semi,
    Pipe,
    AndIf,
    OrIf,
    Background,
    Less,
    Great,
    DGreat,
    ErrGreat,
    ErrDGreat,
    LParen,
    RParen,
    LBrace,
    RBrace,
}

// ===========================================================================
// Public API
// ===========================================================================

/// Parses a full program. `Ok(None)` for blank input.
pub fn parse_program(source: &str) -> Result<Option<Program>, ParseError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    parser.skip_newlines();
    if parser.peek().is_some() {
        return Err(ParseError::Unexpected(format!(
            "unexpected token at end of input: {:?}",
            parser.peek()
        )));
    }
    if program.lists.is_empty() {
        Ok(None)
    } else {
        Ok(Some(program))
    }
}

/// Compatibility: single simple pipeline → legacy [`Pipeline`].
pub fn parse_line(line: &str) -> Result<Option<Pipeline>, ParseError> {
    match parse_program(line)? {
        None => Ok(None),
        Some(program) => Ok(Some(program_to_legacy(program)?)),
    }
}

fn program_to_legacy(program: Program) -> Result<Pipeline, ParseError> {
    if program.lists.len() != 1 || !program.lists[0].rest.is_empty() {
        return Err(ParseError::Unexpected(
            "compound lists require the scripting interpreter".into(),
        ));
    }
    let pipe = &program.lists[0].first;
    let mut commands = Vec::new();
    for cmd in &pipe.commands {
        match cmd {
            Command::Simple(simple) => {
                if !simple.assignments.is_empty() {
                    return Err(ParseError::Unexpected(
                        "assignments require the scripting interpreter".into(),
                    ));
                }
                let mut words: Vec<String> = simple
                    .words
                    .iter()
                    .map(Word::display_raw)
                    .filter(|s| !s.is_empty())
                    .collect();
                if words.is_empty() {
                    return Err(ParseError::EmptyPipelineSegment);
                }
                let program = words.remove(0);
                let redirects = simple
                    .redirects
                    .iter()
                    .map(|r| Redirect {
                        kind: r.kind,
                        target: r.target.display_raw(),
                    })
                    .collect();
                commands.push(SimpleCommand {
                    program,
                    args: words,
                    redirects,
                });
            }
            _ => {
                return Err(ParseError::Unexpected(
                    "compound commands require the scripting interpreter".into(),
                ));
            }
        }
    }
    Ok(Pipeline {
        commands,
        background: pipe.background,
    })
}

// ===========================================================================
// Tokenizer
// ===========================================================================

fn tokenize(source: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut word_parts: Vec<WordPart> = Vec::new();
    let mut unquoted = String::new();

    fn flush_unquoted(parts: &mut Vec<WordPart>, buf: &mut String) {
        if !buf.is_empty() {
            parts.push(WordPart::Unquoted(std::mem::take(buf)));
        }
    }

    fn flush_word(tokens: &mut Vec<Token>, parts: &mut Vec<WordPart>, unquoted: &mut String) {
        flush_unquoted(parts, unquoted);
        if !parts.is_empty() {
            tokens.push(Token::Word(Word {
                parts: std::mem::take(parts),
            }));
        }
    }

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
            }
            '\n' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                tokens.push(Token::Newline);
            }
            '#' if unquoted.is_empty() && word_parts.is_empty() => {
                chars.next();
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch == '\n' {
                        tokens.push(Token::Newline);
                        break;
                    }
                }
            }
            '\'' => {
                chars.next();
                let mut buf = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => buf.push(ch),
                        None => return Err(ParseError::UnterminatedQuote),
                    }
                }
                flush_unquoted(&mut word_parts, &mut unquoted);
                word_parts.push(WordPart::SingleQuoted(buf));
            }
            '"' => {
                chars.next();
                let mut buf = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('\n') => {}
                            Some(next @ ('"' | '\\' | '$' | '`')) => buf.push(next),
                            Some(other) => {
                                buf.push('\\');
                                buf.push(other);
                            }
                            None => return Err(ParseError::UnterminatedQuote),
                        },
                        Some(ch) => buf.push(ch),
                        None => return Err(ParseError::UnterminatedQuote),
                    }
                }
                flush_unquoted(&mut word_parts, &mut unquoted);
                word_parts.push(WordPart::DoubleQuoted(buf));
            }
            '\\' => {
                chars.next();
                match chars.next() {
                    Some('\n') => {}
                    Some(next) => unquoted.push(next),
                    None => return Err(ParseError::TrailingBackslash),
                }
            }
            ';' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                tokens.push(Token::Semi);
            }
            '|' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::OrIf);
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '&' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::AndIf);
                } else {
                    tokens.push(Token::Background);
                }
            }
            '<' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                tokens.push(Token::Less);
            }
            '>' => {
                let is_stderr = word_parts.is_empty() && unquoted == "2";
                if is_stderr {
                    unquoted.clear();
                } else {
                    flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                }
                chars.next();
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
            '(' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                flush_word(&mut tokens, &mut word_parts, &mut unquoted);
                tokens.push(Token::RParen);
            }
            '{' => {
                if unquoted.is_empty() && word_parts.is_empty() {
                    chars.next();
                    tokens.push(Token::LBrace);
                } else {
                    chars.next();
                    unquoted.push('{');
                }
            }
            '}' => {
                if unquoted.is_empty() && word_parts.is_empty() {
                    chars.next();
                    tokens.push(Token::RBrace);
                } else {
                    chars.next();
                    unquoted.push('}');
                }
            }
            _ => {
                chars.next();
                unquoted.push(c);
            }
        }
    }

    flush_word(&mut tokens, &mut word_parts, &mut unquoted);
    Ok(tokens)
}

// ===========================================================================
// Recursive-descent parser
// ===========================================================================

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(Token::Newline)) {
            self.pos += 1;
        }
    }

fn parse_program(&mut self) -> Result<Program, ParseError> {
        self.skip_newlines();
        let mut lists = Vec::new();
        while self.peek().is_some() && !self.at_list_terminator() {
            if matches!(self.peek(), Some(Token::Semi | Token::Newline)) {
                self.bump();
                self.skip_newlines();
                continue;
            }
            // A bare `|` cannot start a list (empty pipeline segment).
            if matches!(self.peek(), Some(Token::Pipe)) {
                return Err(ParseError::EmptyPipelineSegment);
            }
            if !self.can_start_command() {
                break;
            }
            lists.push(self.parse_and_or()?);
            match self.peek() {
                Some(Token::Semi | Token::Newline) => {
                    self.bump();
                    self.skip_newlines();
                }
                None => break,
                Some(_) if self.at_list_terminator() => break,
                Some(_) if self.can_start_command() => continue,
                Some(_) => break,
            }
        }
        Ok(Program { lists })
    }

    fn at_list_terminator(&self) -> bool {
        match self.peek() {
            Some(Token::RParen | Token::RBrace) => true,
            Some(Token::Word(w)) if is_reserved_end(&w.display_raw()) && is_bare_word(w) => true,
            _ => false,
        }
    }

    fn can_start_command(&self) -> bool {
        match self.peek() {
            Some(Token::Word(w)) => {
                let s = w.display_raw();
                !(is_reserved_end(&s) && is_bare_word(w))
            }
            Some(
                Token::LBrace
                | Token::LParen
                | Token::Less
                | Token::Great
                | Token::DGreat
                | Token::ErrGreat
                | Token::ErrDGreat,
            ) => true,
            _ => false,
        }
    }

    fn parse_and_or(&mut self) -> Result<AndOrList, ParseError> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();
        loop {
            let op = match self.peek() {
                Some(Token::AndIf) => AndOrOp::And,
                Some(Token::OrIf) => AndOrOp::Or,
                _ => break,
            };
            self.bump();
            self.skip_newlines();
            if self.peek().is_none() {
                return Err(ParseError::Incomplete);
            }
            rest.push((op, self.parse_pipeline()?));
        }
        Ok(AndOrList { first, rest })
    }

fn parse_pipeline(&mut self) -> Result<AstPipeline, ParseError> {
        // A leading `|` has no left-hand command.
        if matches!(self.peek(), Some(Token::Pipe)) {
            return Err(ParseError::EmptyPipelineSegment);
        }
        let mut commands = vec![self.parse_command()?];
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.bump();
            self.skip_newlines();
            if self.peek().is_none() {
                return Err(ParseError::Incomplete);
            }
            commands.push(self.parse_command()?);
        }
        let background = if matches!(self.peek(), Some(Token::Background)) {
            self.bump();
            true
        } else {
            false
        };
        Ok(AstPipeline {
            commands,
            background,
        })
    }

    fn parse_command(&mut self) -> Result<Command, ParseError> {
        // NAME () compound
        if let Some(Token::Word(name_word)) = self.peek().cloned() {
            let name = name_word.display_raw();
            if is_valid_name(&name)
                && self.tokens.get(self.pos + 1) == Some(&Token::LParen)
                && self.tokens.get(self.pos + 2) == Some(&Token::RParen)
            {
                self.bump();
                self.bump();
                self.bump();
                self.skip_newlines();
                let body = self.parse_compound_required()?;
                return Ok(Command::FunctionDef {
                    name,
                    body: Box::new(body),
                });
            }
        }

        if let Some(c) = self.try_parse_compound()? {
            return Ok(Command::Compound(c));
        }

        Ok(Command::Simple(self.parse_simple_command()?))
    }

    fn parse_compound_required(&mut self) -> Result<CompoundCommand, ParseError> {
        self.try_parse_compound()?.ok_or_else(|| {
            ParseError::Unexpected("function body must be a compound command".into())
        })
    }

    fn try_parse_compound(&mut self) -> Result<Option<CompoundCommand>, ParseError> {
        match self.peek() {
            Some(Token::LBrace) => {
                self.bump();
                self.skip_newlines();
                let body = self.parse_program()?;
                self.skip_newlines();
                match self.bump() {
                    Some(Token::RBrace) => Ok(Some(CompoundCommand::BraceGroup(body))),
                    None => Err(ParseError::Incomplete),
                    _ => Err(ParseError::Unexpected("expected `}'".into())),
                }
            }
            Some(Token::LParen) => {
                self.bump();
                self.skip_newlines();
                let body = self.parse_program()?;
                self.skip_newlines();
                match self.bump() {
                    Some(Token::RParen) => Ok(Some(CompoundCommand::Subshell(body))),
                    None => Err(ParseError::Incomplete),
                    _ => Err(ParseError::Unexpected("expected `)'".into())),
                }
            }
            Some(Token::Word(w)) if is_bare_word(w) && w.display_raw() == "if" => {
                Ok(Some(self.parse_if()?))
            }
            Some(Token::Word(w)) if is_bare_word(w) && w.display_raw() == "while" => {
                Ok(Some(self.parse_while_until(true)?))
            }
            Some(Token::Word(w)) if is_bare_word(w) && w.display_raw() == "until" => {
                Ok(Some(self.parse_while_until(false)?))
            }
            Some(Token::Word(w)) if is_bare_word(w) && w.display_raw() == "for" => {
                Ok(Some(self.parse_for()?))
            }
            Some(Token::Word(w)) if is_bare_word(w) && w.display_raw() == "case" => {
                Ok(Some(self.parse_case()?))
            }
            _ => Ok(None),
        }
    }

    fn parse_if(&mut self) -> Result<CompoundCommand, ParseError> {
        self.bump();
        self.skip_newlines();
        let condition = self.parse_program()?;
        self.skip_newlines();
        self.expect_keyword("then")?;
        self.skip_newlines();
        let then_branch = self.parse_program()?;
        self.skip_newlines();
        let mut elif_branches = Vec::new();
        while self.peek_keyword("elif") {
            self.bump();
            self.skip_newlines();
            let cond = self.parse_program()?;
            self.skip_newlines();
            self.expect_keyword("then")?;
            self.skip_newlines();
            let branch = self.parse_program()?;
            self.skip_newlines();
            elif_branches.push((cond, branch));
        }
        let else_branch = if self.peek_keyword("else") {
            self.bump();
            self.skip_newlines();
            Some(self.parse_program()?)
        } else {
            None
        };
        self.skip_newlines();
        self.expect_keyword("fi")?;
        Ok(CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        })
    }

    fn parse_while_until(&mut self, is_while: bool) -> Result<CompoundCommand, ParseError> {
        self.bump();
        self.skip_newlines();
        let condition = self.parse_program()?;
        self.skip_newlines();
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_program()?;
        self.skip_newlines();
        self.expect_keyword("done")?;
        if is_while {
            Ok(CompoundCommand::While { condition, body })
        } else {
            Ok(CompoundCommand::Until { condition, body })
        }
    }

    fn parse_for(&mut self) -> Result<CompoundCommand, ParseError> {
        self.bump();
        self.skip_newlines();
        let name = match self.bump() {
            Some(Token::Word(w)) => {
                let n = w.display_raw();
                if !is_valid_name(&n) {
                    return Err(ParseError::Unexpected(format!(
                        "`{n}' is not a valid for-loop variable name"
                    )));
                }
                n
            }
            None => return Err(ParseError::Incomplete),
            _ => return Err(ParseError::Unexpected("expected name after `for'".into())),
        };
        self.skip_newlines();
        let mut words = Vec::new();
        if self.peek_keyword("in") {
            self.bump();
            while let Some(Token::Word(_)) = self.peek() {
                if let Some(Token::Word(w)) = self.bump() {
                    words.push(w);
                }
            }
        }
        if matches!(self.peek(), Some(Token::Semi | Token::Newline)) {
            self.bump();
        }
        self.skip_newlines();
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_program()?;
        self.skip_newlines();
        self.expect_keyword("done")?;
        Ok(CompoundCommand::For { name, words, body })
    }

    fn parse_case(&mut self) -> Result<CompoundCommand, ParseError> {
        self.bump();
        self.skip_newlines();
        let word = match self.bump() {
            Some(Token::Word(w)) => w,
            None => return Err(ParseError::Incomplete),
            _ => return Err(ParseError::Unexpected("expected word after `case'".into())),
        };
        self.skip_newlines();
        self.expect_keyword("in")?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while !self.peek_keyword("esac") {
            if self.peek().is_none() {
                return Err(ParseError::Incomplete);
            }
            if matches!(self.peek(), Some(Token::LParen)) {
                self.bump();
            }
            let mut patterns = Vec::new();
            loop {
                match self.bump() {
                    Some(Token::Word(w)) => patterns.push(w),
                    None => return Err(ParseError::Incomplete),
                    _ => {
                        return Err(ParseError::Unexpected(
                            "expected pattern in case arm".into(),
                        ));
                    }
                }
                if matches!(self.peek(), Some(Token::Pipe)) {
                    self.bump();
                    continue;
                }
                break;
            }
            match self.bump() {
                Some(Token::RParen) => {}
                None => return Err(ParseError::Incomplete),
                _ => {
                    return Err(ParseError::Unexpected(
                        "expected `)' after case pattern".into(),
                    ));
                }
            }
            self.skip_newlines();
            let body = self.parse_program()?;
            self.skip_newlines();
            // `;;` = Semi Semi; last arm may omit before esac.
            if matches!(self.peek(), Some(Token::Semi)) {
                self.bump();
                if matches!(self.peek(), Some(Token::Semi)) {
                    self.bump();
                } else if !self.peek_keyword("esac") {
                    return Err(ParseError::Unexpected("expected `;;' after case arm".into()));
                }
            } else if !self.peek_keyword("esac") {
                return Err(ParseError::Unexpected("expected `;;' after case arm".into()));
            }
            self.skip_newlines();
            arms.push(CaseArm { patterns, body });
        }
        self.expect_keyword("esac")?;
        Ok(CompoundCommand::Case { word, arms })
    }

    fn parse_simple_command(&mut self) -> Result<AstSimpleCommand, ParseError> {
        let mut assignments = Vec::new();
        let mut words = Vec::new();
        let mut redirects = Vec::new();
        let mut saw_word = false;

        loop {
            match self.peek().cloned() {
                Some(Token::Word(w)) => {
                    self.bump();
                    if !saw_word {
                        if let Some((name, value)) = split_assignment(&w) {
                            assignments.push(Assignment { name, value });
                            continue;
                        }
                    }
                    saw_word = true;
                    words.push(w);
                }
                Some(
                    Token::Less
                    | Token::Great
                    | Token::DGreat
                    | Token::ErrGreat
                    | Token::ErrDGreat,
                ) => {
                    let kind = match self.bump() {
                        Some(Token::Less) => RedirectKind::Stdin,
                        Some(Token::Great) => RedirectKind::StdoutTruncate,
                        Some(Token::DGreat) => RedirectKind::StdoutAppend,
                        Some(Token::ErrGreat) => RedirectKind::StderrTruncate,
                        Some(Token::ErrDGreat) => RedirectKind::StderrAppend,
                        _ => unreachable!(),
                    };
                    match self.bump() {
                        Some(Token::Word(target)) => {
                            redirects.push(AstRedirect { kind, target });
                        }
                        None => return Err(ParseError::Incomplete),
                        _ => return Err(ParseError::MissingRedirectTarget),
                    }
                }
                _ => break,
            }
        }

        if assignments.is_empty() && words.is_empty() && redirects.is_empty() {
            return Err(match self.peek() {
                None => ParseError::Incomplete,
                _ => ParseError::EmptyPipelineSegment,
            });
        }

        Ok(AstSimpleCommand {
            assignments,
            words,
            redirects,
        })
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        match self.peek() {
            Some(Token::Word(w)) => is_bare_word(w) && w.display_raw() == kw,
            _ => false,
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.peek_keyword(kw) {
            self.bump();
            Ok(())
        } else if self.peek().is_none() {
            Err(ParseError::Incomplete)
        } else {
            Err(ParseError::Unexpected(format!("expected `{kw}'")))
        }
    }
}

fn is_bare_word(w: &Word) -> bool {
    matches!(w.parts.as_slice(), [WordPart::Unquoted(_)])
}

fn is_reserved_end(s: &str) -> bool {
    matches!(
        s,
        "fi" | "done" | "esac" | "then" | "do" | "elif" | "else" | "in"
    )
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn split_assignment(word: &Word) -> Option<(String, Word)> {
    let WordPart::Unquoted(first) = word.parts.first()? else {
        return None;
    };
    let eq = first.find('=')?;
    let name = &first[..eq];
    if !is_valid_name(name) {
        return None;
    }
    let after = &first[eq + 1..];
    let mut value_parts = Vec::new();
    if !after.is_empty() {
        value_parts.push(WordPart::Unquoted(after.to_string()));
    }
    for part in word.parts.iter().skip(1) {
        value_parts.push(part.clone());
    }
    if value_parts.is_empty() {
        value_parts.push(WordPart::Unquoted(String::new()));
    }
    Some((
        name.to_string(),
        Word {
            parts: value_parts,
        },
    ))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        // incomplete rather than empty when nothing follows at EOF
        let err = parse_line("ls |").unwrap_err();
        assert!(matches!(
            err,
            ParseError::Incomplete | ParseError::EmptyPipelineSegment
        ));
    }

    #[test]
    fn doubled_pipe_is_error() {
        // `||` is now OR_IF; `ls || wc` is a valid and-or list for parse_program
        // but parse_line rejects multi-connector lists.
        assert!(parse_line("ls || wc").is_err());
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
        let err = parse_line("echo hi >").unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingRedirectTarget | ParseError::Incomplete
        ));
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
        // `sleep 5 & echo done` is two lists in a program; parse_line rejects.
        assert!(parse_line("sleep 5 & echo done").is_err());
    }

    #[test]
    fn doubled_background_is_error() {
        // `&&` is AND_IF — parse_line rejects and-or lists.
        assert!(parse_line("cmd1 && cmd2").is_err());
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

    #[test]
    fn parse_program_and_or() {
        let prog = parse_program("true && false || true").unwrap().unwrap();
        assert_eq!(prog.lists.len(), 1);
        assert_eq!(prog.lists[0].rest.len(), 2);
        assert_eq!(prog.lists[0].rest[0].0, AndOrOp::And);
        assert_eq!(prog.lists[0].rest[1].0, AndOrOp::Or);
    }

    #[test]
    fn parse_program_if() {
        let prog = parse_program("if true; then echo ok; else echo no; fi")
            .unwrap()
            .unwrap();
        assert_eq!(prog.lists.len(), 1);
        match &prog.lists[0].first.commands[0] {
            Command::Compound(CompoundCommand::If {
                else_branch: Some(_),
                ..
            }) => {}
            other => panic!("expected if, got {other:?}"),
        }
    }

    #[test]
    fn incomplete_if_is_incomplete() {
        let err = parse_program("if true; then echo ok").unwrap_err();
        assert!(err.is_incomplete() || matches!(err, ParseError::Unexpected(_)));
        // missing fi → Incomplete when EOF mid-construct
        assert_eq!(parse_program("if true; then").unwrap_err(), ParseError::Incomplete);
    }

    #[test]
    fn parse_for_loop() {
        let prog = parse_program("for x in a b c; do echo $x; done")
            .unwrap()
            .unwrap();
        match &prog.lists[0].first.commands[0] {
            Command::Compound(CompoundCommand::For { name, words, .. }) => {
                assert_eq!(name, "x");
                assert_eq!(words.len(), 3);
            }
            other => panic!("expected for, got {other:?}"),
        }
    }

    #[test]
    fn parse_function_def() {
        let prog = parse_program("greet() { echo hi; }").unwrap().unwrap();
        match &prog.lists[0].first.commands[0] {
            Command::FunctionDef { name, .. } => assert_eq!(name, "greet"),
            other => panic!("expected function, got {other:?}"),
        }
    }

    #[test]
    fn parse_assignment_prefix() {
        let prog = parse_program("FOO=bar echo hi").unwrap().unwrap();
        match &prog.lists[0].first.commands[0] {
            Command::Simple(s) => {
                assert_eq!(s.assignments.len(), 1);
                assert_eq!(s.assignments[0].name, "FOO");
                assert_eq!(s.words[0].display_raw(), "echo");
            }
            other => panic!("expected simple, got {other:?}"),
        }
    }

    #[test]
    fn incomplete_error_flag() {
        assert!(ParseError::Incomplete.is_incomplete());
        assert!(ParseError::UnterminatedQuote.is_incomplete());
        assert!(!ParseError::EmptyPipelineSegment.is_incomplete());
    }
}
