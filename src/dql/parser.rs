use crate::dql::ast::*;
use crate::dql::token::Token;

pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) {
        self.position += 1;
    }

    fn expect(&mut self, expected: Token) -> Result<(), String> {
        if *self.current() == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!(
                "Expected {:?}, found {:?}",
                expected,
                self.current()
            ))
        }
    }

    fn parse_string(&mut self, error: &str) -> Result<String, String> {
        match self.current() {
            Token::String(value) => {
                let value = value.clone();
                self.advance();
                Ok(value)
            }

            _ => Err(error.to_string()),
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        match self.current() {
            Token::String(value) => {
                let value = value.clone();
                self.advance();
                Ok(Value::String(value))
            }

            Token::Number(value) => {
                let value = value.clone();
                self.advance();
                Ok(Value::Number(value))
            }

            Token::Boolean(value) => {
                let value = *value;
                self.advance();
                Ok(Value::Boolean(value))
            }

            Token::Variable(value) => {
                let value = value.clone();
                self.advance();
                Ok(Value::Variable(value))
            }

            Token::LBracket => self.parse_vector_value(),

            _ => Err(format!("Expected value, found {:?}", self.current())),
        }
    }

    fn parse_vector_value(&mut self) -> Result<Value, String> {
        self.expect(Token::LBracket)?;

        let mut values = Vec::new();

        loop {
            match self.current() {
                Token::Number(value) => {
                    values.push(value.clone());
                    self.advance();
                }
                Token::RBracket => {
                    self.advance();
                    break;
                }
                _ => return Err("Expected number or ']'".to_string()),
            }

            match self.current() {
                Token::Comma => {
                    self.advance();
                }
                Token::RBracket => {
                    self.advance();
                    break;
                }
                _ => return Err("Expected ',' or ']'".to_string()),
            }
        }

        Ok(Value::Vector(values))
    }

    pub fn parse(&mut self) -> Result<Statement, String> {
        let statement = match self.current() {
            Token::Create => self.parse_create(),
            Token::Drop => self.parse_drop(),
            Token::Insert => self.parse_insert(),
            Token::Update => self.parse_update(),
            Token::Delete => self.parse_delete(),
            Token::Find => self.parse_find(),

            Token::Eof => Err("Expected statement".to_string()),

            _ => Err("Unexpected token".to_string()),
        }?;

        if !matches!(self.current(), Token::Eof) {
            return Err(format!("Unexpected trailing token {:?}", self.current()));
        }

        Ok(statement)
    }

    fn parse_create(&mut self) -> Result<Statement, String> {
        match self.current() {
            Token::Create => self.advance(),
            _ => return Err("Expected 'create'".to_string()),
        }

        match self.current() {
            Token::Collection => self.parse_create_collection_after_keyword(),

            Token::Index => self.parse_create_index_after_keyword(),

            _ => Err("Expected 'collection' or 'index'".to_string()),
        }
    }

    fn parse_create_collection_after_keyword(&mut self) -> Result<Statement, String> {
        self.expect(Token::Collection)?;

        let name = self.parse_string("Expected collection name")?;

        self.expect(Token::LBrace)?;

        let mut fields = Vec::new();

        while !matches!(self.current(), Token::RBrace | Token::Eof) {
            fields.push(self.parse_field()?);

            match self.current() {
                Token::Comma => {
                    self.advance();
                }

                Token::RBrace => {
                    break;
                }

                _ => {
                    return Err("Expected ',' or '}'".to_string());
                }
            }
        }

        self.expect(Token::RBrace)?;
        self.expect(Token::Semicolon)?;

        Ok(Statement::CreateCollection(CreateCollection {
            name,
            fields,
        }))
    }

    fn parse_create_index_after_keyword(&mut self) -> Result<Statement, String> {
        self.expect(Token::Index)?;

        self.expect(Token::On)?;

        let collection = self.parse_string("Expected collection name")?;

        self.expect(Token::LParen)?;

        let mut fields = Vec::new();

        loop {
            fields.push(self.parse_string("Expected field name")?);

            match self.current() {
                Token::Comma => {
                    self.advance();
                }

                Token::RParen => {
                    self.advance();
                    break;
                }

                _ => {
                    return Err("Expected ',' or ')'".to_string());
                }
            }
        }

        self.expect(Token::Using)?;

        let index_type = match self.current() {
            Token::Identifier(value) => {
                let value = value.clone();
                self.advance();
                value
            }

            _ => {
                return Err("Expected index type".to_string());
            }
        };

        let properties = if matches!(self.current(), Token::With) {
            self.parse_index_properties()?
        } else {
            Vec::new()
        };

        self.expect(Token::Semicolon)?;

        Ok(Statement::CreateIndex(CreateIndex {
            collection,
            fields,
            index_type,
            properties,
        }))
    }

    fn parse_index_properties(&mut self) -> Result<Vec<IndexProperty>, String> {
        self.expect(Token::With)?;
        self.expect(Token::LParen)?;

        let mut properties = Vec::new();

        loop {
            let name = match self.current() {
                Token::Identifier(value) => {
                    let value = value.clone();
                    self.advance();
                    value
                }
                _ => return Err("Expected index property name".to_string()),
            };

            self.expect(Token::Equal)?;

            let value = self.parse_value()?;

            properties.push(IndexProperty { name, value });

            match self.current() {
                Token::Comma => {
                    self.advance();
                }
                Token::RParen => {
                    self.advance();
                    break;
                }
                _ => return Err("Expected ',' or ')'".to_string()),
            }
        }

        Ok(properties)
    }

    fn parse_drop(&mut self) -> Result<Statement, String> {
        self.expect(Token::Drop)?;

        match self.current() {
            Token::Collection => {
                self.advance();
                self.parse_drop_collection()
            }

            Token::Index => {
                self.advance();
                self.parse_drop_index()
            }

            _ => Err("Expected 'collection' or 'index'".to_string()),
        }
    }

    fn parse_drop_collection(&mut self) -> Result<Statement, String> {
        let name = self.parse_string("Expected collection name")?;

        self.expect(Token::Semicolon)?;

        Ok(Statement::DropCollection(DropCollection { name }))
    }

    fn parse_drop_index(&mut self) -> Result<Statement, String> {
        self.expect(Token::On)?;

        let collection = self.parse_string("Expected collection name")?;

        self.expect(Token::LParen)?;

        let field = self.parse_string("Expected field name")?;

        self.expect(Token::RParen)?;
        self.expect(Token::Semicolon)?;

        Ok(Statement::DropIndex(DropIndex { collection, field }))
    }

    fn parse_insert(&mut self) -> Result<Statement, String> {
        self.expect(Token::Insert)?;
        self.expect(Token::Into)?;

        let collection = self.parse_string("Expected collection name")?;

        self.expect(Token::LBrace)?;

        let mut values = Vec::new();

        while !matches!(self.current(), Token::RBrace | Token::Eof) {
            let field = self.parse_string("Expected field name")?;

            self.expect(Token::Colon)?;

            let value = self.parse_value()?;

            values.push(FieldValue { field, value });

            match self.current() {
                Token::Comma => {
                    self.advance();
                }

                Token::RBrace => {
                    break;
                }

                _ => {
                    return Err("Expected ',' or '}'".to_string());
                }
            }
        }

        self.expect(Token::RBrace)?;
        self.expect(Token::Semicolon)?;

        Ok(Statement::Insert(Insert { collection, values }))
    }

    fn parse_update(&mut self) -> Result<Statement, String> {
        self.expect(Token::Update)?;

        let collection = self.parse_string("Expected collection name")?;

        self.expect(Token::Set)?;

        let field = self.parse_string("Expected field name")?;

        self.expect(Token::Equal)?;

        let value = self.parse_value()?;

        let filter = self.parse_filter()?;

        self.expect(Token::Semicolon)?;

        Ok(Statement::Update(Update {
            collection,
            field,
            value,
            filter,
        }))
    }

    fn parse_filter(&mut self) -> Result<FilterExpr, String> {
        self.expect(Token::Filter)?;
        self.expect(Token::On)?;

        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<FilterExpr, String> {
        let mut expr = self.parse_and()?;

        while matches!(self.current(), Token::Or) {
            self.advance();

            let right = self.parse_and()?;

            expr = FilterExpr::Or(Box::new(expr), Box::new(right));
        }

        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<FilterExpr, String> {
        let mut expr = self.parse_comparison()?;

        while matches!(self.current(), Token::And) {
            self.advance();

            let right = self.parse_comparison()?;

            expr = FilterExpr::And(Box::new(expr), Box::new(right));
        }

        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<FilterExpr, String> {
        let field = self.parse_string("Expected filter field")?;

        let operator = match self.current() {
            Token::Equal => {
                self.advance();
                Operator::Equal
            }

            Token::NotEqual => {
                self.advance();
                Operator::NotEqual
            }

            Token::Less => {
                self.advance();
                Operator::Less
            }

            Token::Greater => {
                self.advance();
                Operator::Greater
            }

            Token::LessEqual => {
                self.advance();
                Operator::LessEqual
            }

            Token::GreaterEqual => {
                self.advance();
                Operator::GreaterEqual
            }

            _ => return Err("Expected comparison operator".to_string()),
        };

        let value = self.parse_value()?;

        Ok(FilterExpr::Comparison {
            field,
            operator,
            value,
        })
    }

    fn parse_delete(&mut self) -> Result<Statement, String> {
        self.expect(Token::Delete)?;
        self.expect(Token::From)?;

        let collection = self.parse_string("Expected collection name")?;

        let filter = self.parse_filter()?;

        self.expect(Token::Semicolon)?;

        Ok(Statement::Delete(Delete { collection, filter }))
    }

    fn parse_find(&mut self) -> Result<Statement, String> {
        self.expect(Token::Find)?;

        let top = self.parse_top_clause()?;

        self.expect(Token::Near)?;

        let vector = self.parse_vector_expr()?;

        self.expect(Token::On)?;

        let collection = self.parse_string("Expected collection name")?;

        self.expect(Token::LParen)?;

        let column = self.parse_string("Expected vector column")?;

        self.expect(Token::RParen)?;

        let filter = if matches!(self.current(), Token::Filter) {
            Some(self.parse_filter()?)
        } else {
            None
        };

        let search = if matches!(self.current(), Token::Search) {
            Some(self.parse_search()?)
        } else {
            None
        };

        self.expect(Token::Return)?;

        let return_fields = self.parse_return_fields()?;

        self.expect(Token::Semicolon)?;

        Ok(Statement::Find(Find {
            top,
            vector,
            collection,
            column,
            filter,
            search,
            return_fields,
        }))
    }

    fn parse_top_clause(&mut self) -> Result<TopClause, String> {
        self.expect(Token::Top)?;

        let limit = match self.current() {
            Token::Number(value) => {
                let value = value.clone();
                self.advance();

                value
                    .parse::<u64>()
                    .map_err(|_| "Invalid top value".to_string())?
            }
            _ => return Err("Expected integer after 'top'".to_string()),
        };

        let within = if matches!(self.current(), Token::Within) {
            self.advance();

            match self.current() {
                Token::Number(value) => {
                    let value = value.clone();
                    self.advance();
                    Some(value)
                }
                _ => return Err("Expected value after 'within'".to_string()),
            }
        } else {
            None
        };

        Ok(TopClause { limit, within })
    }

    fn parse_vector_expr(&mut self) -> Result<VectorExpr, String> {
        match self.current() {
            Token::Variable(name) => {
                let name = name.clone();
                self.advance();

                Ok(VectorExpr::Variable(name))
            }

            Token::LBracket => {
                self.advance();

                let mut values = Vec::new();

                loop {
                    match self.current() {
                        Token::Number(value) => {
                            values.push(value.clone());
                            self.advance();
                        }

                        Token::RBracket => {
                            self.advance();
                            break;
                        }

                        _ => {
                            return Err("Expected vector value or ']'".to_string());
                        }
                    }

                    match self.current() {
                        Token::Comma => {
                            self.advance();
                        }

                        Token::RBracket => {
                            self.advance();
                            break;
                        }

                        _ => {
                            return Err("Expected ',' or ']'".to_string());
                        }
                    }
                }

                Ok(VectorExpr::Literal(values))
            }

            _ => Err("Expected vector literal or variable".to_string()),
        }
    }

    fn parse_search(&mut self) -> Result<SearchClause, String> {
        self.expect(Token::Search)?;

        let property = match self.current() {
            Token::Identifier(value) => {
                let value = value.clone();
                self.advance();
                value
            }

            _ => return Err("Expected search property".to_string()),
        };

        let value = match self.current() {
            Token::Number(value) => {
                let value = value.clone();
                self.advance();

                value
                    .parse::<u64>()
                    .map_err(|_| "Invalid search value".to_string())?
            }

            _ => return Err("Expected integer search value".to_string()),
        };

        Ok(SearchClause { property, value })
    }

    fn parse_return_fields(&mut self) -> Result<Vec<ReturnField>, String> {
        let mut fields = Vec::new();

        loop {
            match self.current() {
                Token::String(name) => {
                    let name = name.clone();
                    self.advance();

                    fields.push(ReturnField::Field(name));
                }

                Token::Identifier(name) if name == "_score" => {
                    self.advance();

                    fields.push(ReturnField::Score);
                }

                _ => {
                    return Err("Expected return field".to_string());
                }
            }

            match self.current() {
                Token::Comma => {
                    self.advance();
                }

                Token::Semicolon => {
                    break;
                }

                _ => {
                    return Err("Expected ',' or ';'".to_string());
                }
            }
        }

        Ok(fields)
    }

    fn parse_field(&mut self) -> Result<Field, String> {
        let name = match self.current() {
            Token::String(name) => {
                let name = name.clone();
                self.advance();
                name
            }

            _ => return Err("Expected field name".to_string()),
        };

        let data_type = self.parse_type()?;

        let mut properties = Vec::new();

        while let Token::Identifier(value) = self.current() {
            properties.push(value.clone());
            self.advance();
        }

        Ok(Field {
            name,
            data_type,
            properties,
        })
    }

    fn parse_type(&mut self) -> Result<String, String> {
        let mut data_type = match self.current() {
            Token::Identifier(value) => {
                let value = value.clone();
                self.advance();
                value
            }

            _ => return Err("Expected data type".to_string()),
        };

        // vector<fp32, 768>
        if matches!(self.current(), Token::Less) {
            data_type.push('<');
            self.advance();

            loop {
                match self.current() {
                    Token::Identifier(value) => {
                        data_type.push_str(value);
                        self.advance();
                    }

                    Token::Number(value) => {
                        data_type.push_str(value);
                        self.advance();
                    }

                    _ => {
                        return Err("Expected type parameter".to_string());
                    }
                }

                match self.current() {
                    Token::Comma => {
                        data_type.push(',');
                        self.advance();
                    }

                    Token::Greater => {
                        data_type.push('>');
                        self.advance();
                        break;
                    }

                    _ => {
                        return Err("Expected ',' or '>'".to_string());
                    }
                }
            }
        }

        Ok(data_type)
    }
}
