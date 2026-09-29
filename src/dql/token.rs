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
    Top,
    Within,
    Near,
    Search,
    Return,

    Filter,

    // Logical operators
    And,
    Or,

    // Column properties
    Primary,
    AutoIncrement,
    Nullable,

    // Index types
    Flat,
    Hnsw,
    Ivf,

    // Search properties
    Ef,
    Nprobe,

    // Vector metrics
    Cosine,
    DotProduct,
    Euclidean,

    // Literals / values
    Identifier(String),
    String(String),
    Number(String),
    Boolean(bool),
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
