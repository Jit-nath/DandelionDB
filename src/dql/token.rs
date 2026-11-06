#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Create,
    Collection,
    Drop,

    Index,
    On,
    Using,
    With,

    Insert,
    Into,

    Update,
    Set,

    Delete,
    From,

    Find,
    Get,
    Check,
    Stats,
    Show,
    Segments,
    Database,
    Top,
    Within,
    Near,
    Search,
    Return,

    Filter,

    // Logical operators
    And,
    Or,

    // Literals / values
    Identifier(String),
    String(String),
    Number(String),
    Boolean(bool),
    Null,
    Variable(String),

    // Symbols
    LBrace,   // {
    RBrace,   // }
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]

    Comma,     // ,
    Colon,     // :
    Semicolon, // ;

    Less,         // <
    Greater,      // >
    LessEqual,    // <=
    GreaterEqual, // >=
    Equal,        // =
    NotEqual,     // !=

    // End
    Eof,
}
