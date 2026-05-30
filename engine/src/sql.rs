use crate::{EngineError, Result};

#[derive(Debug, Eq, PartialEq)]
pub enum Statement {
    Get { key: String },
    Set { key: String, value: String },
}

pub fn parse(input: &str) -> Result<Statement> {
    let mut parts = input.split_whitespace();
    let command = parts
        .next()
        .ok_or_else(|| EngineError::InvalidStatement(input.to_owned()))?;

    match command.to_ascii_uppercase().as_str() {
        "GET" => {
            let key = expect_one_arg(input, parts)?;
            Ok(Statement::Get { key })
        }
        "SET" => {
            let key = parts
                .next()
                .ok_or_else(|| EngineError::InvalidStatement(input.to_owned()))?;
            let value = parts
                .next()
                .ok_or_else(|| EngineError::InvalidStatement(input.to_owned()))?;

            if parts.next().is_some() {
                return Err(EngineError::InvalidStatement(input.to_owned()));
            }

            Ok(Statement::Set {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        }
        _ => Err(EngineError::InvalidStatement(input.to_owned())),
    }
}

fn expect_one_arg<'a>(input: &str, mut parts: impl Iterator<Item = &'a str>) -> Result<String> {
    let key = parts
        .next()
        .ok_or_else(|| EngineError::InvalidStatement(input.to_owned()))?;

    if parts.next().is_some() {
        return Err(EngineError::InvalidStatement(input.to_owned()));
    }

    Ok(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Statement, parse};

    #[test]
    fn parses_set_statement() {
        assert_eq!(
            parse("SET flower dandelion").unwrap(),
            Statement::Set {
                key: "flower".to_owned(),
                value: "dandelion".to_owned(),
            }
        );
    }

    #[test]
    fn parses_get_statement() {
        assert_eq!(
            parse("GET flower").unwrap(),
            Statement::Get {
                key: "flower".to_owned(),
            }
        );
    }
}
