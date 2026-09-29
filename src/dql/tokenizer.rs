use crate::dql::token::Token;

pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\n' | '\t' | '\r' => {
                chars.next();
            }

            '{' => {
                chars.next();
                tokens.push(Token::LBrace);
            }

            '}' => {
                chars.next();
                tokens.push(Token::RBrace);
            }

            ',' => {
                chars.next();
                tokens.push(Token::Comma);
            }

            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }

            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }

            '[' => {
                chars.next();
                tokens.push(Token::LBracket);
            }

            ']' => {
                chars.next();
                tokens.push(Token::RBracket);
            }

            ':' => {
                chars.next();
                tokens.push(Token::Colon);
            }

            ';' => {
                chars.next();
                tokens.push(Token::Semicolon);
            }

            '"' => {
                chars.next();

                let mut value = String::new();
                let mut closed = false;

                while let Some(&ch) = chars.peek() {
                    chars.next();

                    if ch == '"' {
                        closed = true;
                        break;
                    }

                    value.push(ch);
                }

                if !closed {
                    return Err("Unterminated string".to_string());
                }

                tokens.push(Token::String(value));
            }

            '<' => {
                chars.next();

                if let Some('=') = chars.peek() {
                    chars.next();
                    tokens.push(Token::LessEqual);
                } else {
                    tokens.push(Token::Less);
                }
            }

            '>' => {
                chars.next();

                if let Some('=') = chars.peek() {
                    chars.next();
                    tokens.push(Token::GreaterEqual);
                } else {
                    tokens.push(Token::Greater);
                }
            }

            '=' => {
                chars.next();
                tokens.push(Token::Equal);
            }

            '!' => {
                chars.next();

                if let Some('=') = chars.peek() {
                    chars.next();
                    tokens.push(Token::NotEqual);
                } else {
                    return Err("Unexpected character: !".to_string());
                }
            }

            '/' => {
                chars.next();

                if let Some('/') = chars.peek() {
                    chars.next();

                    while let Some(&ch) = chars.peek() {
                        chars.next();

                        if ch == '\n' {
                            break;
                        }
                    }
                } else {
                    return Err("Unexpected character: /".to_string());
                }
            }

            c if c.is_ascii_digit() => {
                let mut value = String::new();

                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() || ch == '.' {
                        value.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }

                tokens.push(Token::Number(value));
            }

            '$' => {
                chars.next();

                let mut name = String::new();

                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }

                if name.is_empty() {
                    return Err("Expected variable name after $".to_string());
                }

                tokens.push(Token::Variable(name));
            }

            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut word = String::new();

                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        word.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }

                match word.as_str() {
                    "create" => tokens.push(Token::Create),
                    "collection" => tokens.push(Token::Collection),
                    "drop" => tokens.push(Token::Drop),

                    "index" => tokens.push(Token::Index),
                    "on" => tokens.push(Token::On),
                    "using" => tokens.push(Token::Using),
                    "with" => tokens.push(Token::With),

                    "insert" => tokens.push(Token::Insert),
                    "into" => tokens.push(Token::Into),

                    "update" => tokens.push(Token::Update),
                    "set" => tokens.push(Token::Set),

                    "delete" => tokens.push(Token::Delete),
                    "from" => tokens.push(Token::From),

                    "find" => tokens.push(Token::Find),
                    "top" => tokens.push(Token::Top),
                    "within" => tokens.push(Token::Within),
                    "near" => tokens.push(Token::Near),
                    "search" => tokens.push(Token::Search),
                    "return" => tokens.push(Token::Return),

                    "filter" => tokens.push(Token::Filter),

                    "and" => tokens.push(Token::And),
                    "or" => tokens.push(Token::Or),

                    "true" => tokens.push(Token::Boolean(true)),
                    "false" => tokens.push(Token::Boolean(false)),

                    _ => tokens.push(Token::Identifier(word)),
                }
            }

            _ => {
                return Err(format!("Unexpected character: {}", c));
            }
        }
    }

    tokens.push(Token::Eof);

    Ok(tokens)
}
