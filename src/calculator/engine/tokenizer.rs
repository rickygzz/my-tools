//! Splits a line into tokens.
//!
//! ```text
//! Number     ::= Digit+ ('.' Digit*)? | '.' Digit+
//! Time       ::= Digit+ ':' Digit+
//! Date       ::= Digit{4} '-' Digit{1,2} '-' Digit{1,2}   (not 2026 minus 12 minus 25)
//! Identifier ::= (Letter | '_') (Letter | Digit | '_')*
//! Text       ::= '"' (Char | '""')* '"'                 ("" inside the text is one quote)
//! Symbol     ::= one of + - * / % ^ ( ) , =
//! ```
//!
//! A '-' is always its own token; the parser decides whether it subtracts or negates.
//! Positions count characters (not bytes) from 0, so the window can select them.

use super::{CalcError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Number,
    /// 10:30
    Time,
    /// 2026-12-25
    Date,
    /// "text", without its quotes.
    Text,
    Identifier,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    Power,
    LeftParenthesis,
    RightParenthesis,
    Comma,
    /// The '=' that gives a value a name: x = 5.
    Assign,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub value: String,
    pub position: usize,
}

impl Token {
    fn new(kind: Kind, value: impl Into<String>, position: usize) -> Self {
        Self { kind, value: value.into(), position }
    }
}

/// The line's tokens. The last one is always [`Kind::End`].
pub fn tokenize(line: &str) -> Result<Vec<Token>> {
    let mut tokenizer = Tokenizer { chars: line.chars().collect(), position: 0 };
    let mut tokens = Vec::new();

    loop {
        tokenizer.skip_whitespace();
        let Some(c) = tokenizer.current() else { break };

        let token = if c.is_ascii_digit() || c == '.' {
            tokenizer.read_number()?
        } else if c.is_ascii_alphabetic() || c == '_' {
            tokenizer.read_identifier()
        } else if c == '"' {
            tokenizer.read_text()?
        } else {
            let kind = match c {
                '+' => Kind::Plus,
                '-' => Kind::Minus,
                '*' => Kind::Multiply,
                '/' => Kind::Divide,
                '%' => Kind::Modulo,
                '^' => Kind::Power,
                '(' => Kind::LeftParenthesis,
                ')' => Kind::RightParenthesis,
                ',' => Kind::Comma,
                '=' => Kind::Assign,
                _ => {
                    return Err(CalcError::new(
                        format!("Invalid character '{c}'"),
                        tokenizer.position,
                    ));
                }
            };
            let token = Token::new(kind, c, tokenizer.position);
            tokenizer.position += 1;
            token
        };
        tokens.push(token);
    }

    tokens.push(Token::new(Kind::End, "", tokenizer.position));
    Ok(tokens)
}

/// A number as typed (`1.`, `.5`), as a `Decimal`; `None` when it doesn't fit in one.
pub fn parse_number(text: &str) -> Option<rust_decimal::Decimal> {
    text.parse().ok()
}

struct Tokenizer {
    chars: Vec<char>,
    position: usize,
}

impl Tokenizer {
    fn current(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn at(&self, position: usize) -> Option<char> {
        self.chars.get(position).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.current().is_some_and(char::is_whitespace) {
            self.position += 1;
        }
    }

    fn text(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect()
    }

    fn read_number(&mut self) -> Result<Token> {
        let start = self.position;
        let mut has_point = false;

        while let Some(c) = self.current()
            && (c.is_ascii_digit() || c == '.')
        {
            if c == '.' {
                if has_point {
                    return Err(CalcError::new(
                        "Invalid number format: multiple decimal points",
                        self.position,
                    ));
                }
                has_point = true;
            }
            self.position += 1;
        }
        let number = self.text(start, self.position);

        // Four digits, then -month-day, is a date.
        if !has_point
            && number.len() == 4
            && let Some(length) = self.date_rest_length()
        {
            self.position += length;
            return Ok(Token::new(Kind::Date, self.text(start, self.position), start));
        }

        if self.current() == Some(':') && !has_point {
            return self.read_time(start);
        }

        if number == "." {
            return Err(CalcError::new("Invalid number format '.'", start));
        }

        // Only digits and one '.' are left, so failing means it doesn't fit in a Decimal.
        if parse_number(&number).is_none() {
            return Err(CalcError::new(format!("Number is too large '{number}'"), start));
        }

        Ok(Token::new(Kind::Number, number, start))
    }

    /// The length of the `-12-25` of `2026-12-25`, right where the year ends; `None` when what
    /// follows isn't that, or is part of a longer number (`2026-12-255`, `2026-1-5.5`).
    fn date_rest_length(&self) -> Option<usize> {
        let mut position = self.position;
        for part in 0..2 {
            if self.at(position) != Some('-') {
                return None;
            }
            position += 1;
            let digits_start = position;
            while self.at(position).is_some_and(|c| c.is_ascii_digit()) {
                position += 1;
            }
            let digits = position - digits_start;
            if digits == 0 || digits > 2 {
                return None;
            }
            if part == 1 && matches!(self.at(position), Some('.' | ':')) {
                return None;
            }
        }
        Some(position - self.position)
    }

    /// Reads the minutes of a time such as 10:30, after its hours. The parser checks the values.
    fn read_time(&mut self, start: usize) -> Result<Token> {
        self.position += 1; // the ':'
        let minutes_start = self.position;
        while self.current().is_some_and(|c| c.is_ascii_digit()) {
            self.position += 1;
        }
        if self.position == minutes_start {
            return Err(CalcError::new("Expected minutes after ':'", self.position));
        }
        Ok(Token::new(Kind::Time, self.text(start, self.position), start))
    }

    fn read_identifier(&mut self) -> Token {
        let start = self.position;
        while self.current().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.position += 1;
        }
        Token::new(Kind::Identifier, self.text(start, self.position), start)
    }

    /// Reads text in double quotes. The token's value is the text without the quotes, with
    /// `""` turned into `"`.
    fn read_text(&mut self) -> Result<Token> {
        let start = self.position;
        let mut text = String::new();
        self.position += 1; // the opening quote

        loop {
            let Some(c) = self.current() else {
                return Err(CalcError::new("Text is missing its closing quote", start));
            };
            if c == '"' {
                if self.at(self.position + 1) != Some('"') {
                    self.position += 1; // the closing quote
                    break;
                }
                self.position += 1; // the first quote of ""; the second is added below
            }
            text.push(c);
            self.position += 1;
        }

        Ok(Token::new(Kind::Text, text, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(line: &str) -> Vec<(Kind, String, usize)> {
        tokenize(line)
            .unwrap()
            .into_iter()
            .map(|token| (token.kind, token.value, token.position))
            .collect()
    }

    fn t(kind: Kind, value: &str, position: usize) -> (Kind, String, usize) {
        (kind, value.to_string(), position)
    }

    use Kind::*;

    #[test]
    fn numbers() {
        assert_eq!(tokens(""), [t(End, "", 0)]);
        for digit in 0..=9 {
            let text = digit.to_string();
            assert_eq!(tokens(&text), [t(Number, &text, 0), t(End, "", 1)]);
        }
        assert_eq!(tokens("1.1"), [t(Number, "1.1", 0), t(End, "", 3)]);
        assert_eq!(tokens("1."), [t(Number, "1.", 0), t(End, "", 2)]);
        assert_eq!(tokens(".5"), [t(Number, ".5", 0), t(End, "", 2)]);
    }

    #[test]
    fn operators() {
        for (line, kind, symbol) in [
            ("1+2", Plus, "+"),
            ("3-2", Minus, "-"),
            ("2*3", Multiply, "*"),
            ("6/3", Divide, "/"),
            ("7%3", Modulo, "%"),
            ("2^3", Power, "^"),
        ] {
            let (left, right) = (&line[0..1], &line[2..3]);
            assert_eq!(
                tokens(line),
                [t(Number, left, 0), t(kind, symbol, 1), t(Number, right, 2), t(End, "", 3)]
            );
        }
        assert_eq!(tokens("-2"), [t(Minus, "-", 0), t(Number, "2", 1), t(End, "", 2)]);
        assert_eq!(
            tokens("(1)"),
            [t(LeftParenthesis, "(", 0), t(Number, "1", 1), t(RightParenthesis, ")", 2), t(End, "", 3)]
        );
    }

    #[test]
    fn whitespace() {
        assert_eq!(tokens("   "), [t(End, "", 3)]);
        assert_eq!(tokens(" 1"), [t(Number, "1", 1), t(End, "", 2)]);
        assert_eq!(tokens("1 "), [t(Number, "1", 0), t(End, "", 2)]);
        assert_eq!(
            tokens("1 + 2"),
            [t(Number, "1", 0), t(Plus, "+", 2), t(Number, "2", 4), t(End, "", 5)]
        );
        assert_eq!(tokens("\t1\r\n"), [t(Number, "1", 1), t(End, "", 4)]);
    }

    #[test]
    fn identifiers() {
        assert_eq!(tokens("pi"), [t(Identifier, "pi", 0), t(End, "", 2)]);
        assert_eq!(tokens("_x1"), [t(Identifier, "_x1", 0), t(End, "", 3)]);
        assert_eq!(tokens("2pi"), [t(Number, "2", 0), t(Identifier, "pi", 1), t(End, "", 3)]);
        assert_eq!(
            tokens("max(1, 2)"),
            [
                t(Identifier, "max", 0),
                t(LeftParenthesis, "(", 3),
                t(Number, "1", 4),
                t(Comma, ",", 5),
                t(Number, "2", 7),
                t(RightParenthesis, ")", 8),
                t(End, "", 9),
            ]
        );
    }

    #[test]
    fn texts() {
        assert_eq!(tokens(r#""abc""#), [t(Text, "abc", 0), t(End, "", 5)]);
        assert_eq!(tokens(r#""""#), [t(Text, "", 0), t(End, "", 2)]);
        assert_eq!(tokens(r#""a""b""#), [t(Text, "a\"b", 0), t(End, "", 6)]);
        assert_eq!(tokens(r#""""""#), [t(Text, "\"", 0), t(End, "", 4)]);
        assert_eq!(tokens(r#"" a+b, (c) ""#), [t(Text, " a+b, (c) ", 0), t(End, "", 12)]);
        assert_eq!(
            tokens(r#"len("hi")"#),
            [
                t(Identifier, "len", 0),
                t(LeftParenthesis, "(", 3),
                t(Text, "hi", 4),
                t(RightParenthesis, ")", 8),
                t(End, "", 9),
            ]
        );
    }

    #[test]
    fn times_and_dates() {
        assert_eq!(
            tokens("10:30pm"),
            [t(Time, "10:30", 0), t(Identifier, "pm", 5), t(End, "", 7)]
        );
        assert_eq!(tokens("2026-12-25"), [t(Date, "2026-12-25", 0), t(End, "", 10)]);
        assert_eq!(tokens("2026-1-5"), [t(Date, "2026-1-5", 0), t(End, "", 8)]);
        // Spaced out, or not shaped like a date, it's a subtraction.
        assert_eq!(tokens("2026 - 12 - 25").len(), 6);
        assert_eq!(tokens("2026-12-255").len(), 6);
        assert_eq!(tokens("12026-12-25").len(), 6);
    }

    #[test]
    fn invalid_input_says_where() {
        for (line, position) in [
            ("1 $ 2", 2),
            (r#""abc"#, 0),
            (r#"len("abc)"#, 4),
            (r#""a"""#, 0),
            ("1.2.3", 3),
            (".", 0),
            ("2 + .", 4),
            ("é", 0),
            ("1 + 79228162514264337593543950336", 4),
            ("10:", 3),
            ("10:x", 3),
            ("1.5:30", 3),
            (":30", 0),
        ] {
            assert_eq!(tokenize(line).unwrap_err().position, position, "{line}");
        }
    }
}
