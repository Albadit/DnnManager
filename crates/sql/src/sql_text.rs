//! Text put into T-SQL where a parameter can't go (`SqlText`), and DNN's table names (`DnnTables`). Database names
//! aren't always DNN Manager's own - the one a site uses is read out of its web.config, the logical file names out of a
//! backup - so they are quoted, never put in as they are.

/// `name` as an identifier: `[` + the name with `]` doubled + `]`.
pub fn identifier(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// `value` for inside a string literal (`N'…'`): `'` doubled.
pub fn literal(value: &str) -> String {
    value.replace('\'', "''")
}

/// The characters LIKE reads as wildcards (`[`, `%` and `_`) as themselves.
pub fn escape_like(value: &str) -> String {
    value.replace('[', "[[]").replace('%', "[%]").replace('_', "[_]")
}

/// DNN's tables can start with an `objectQualifier` (`dnn_Users`), found from the tables themselves; a qualifier is
/// letters, digits and `_` - anything else isn't DNN's and never goes into SQL text (`DnnTables`).
pub mod dnn_tables {
    /// The query that finds the qualifier of the DNN table `@P1` (a constant such as "Version").
    pub const QUALIFIER_SQL: &str = "SELECT TOP 1 LEFT(name, LEN(name) - LEN(@P1)) FROM sys.tables WHERE SCHEMA_NAME(schema_id) = 'dbo' \
         AND (name = @P1 OR name LIKE '%[_]' + @P1) ORDER BY LEN(name)";

    /// The table's name to put in SQL: `dbo.[dnn_Users]`.
    pub fn name(qualifier: &str, table: &str) -> String {
        format!("dbo.[{qualifier}{table}]")
    }

    pub fn is_qualifier(text: &str) -> bool {
        text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_quoted() {
        assert_eq!(identifier("shop"), "[shop]");
        assert_eq!(identifier("a]b"), "[a]]b]");
        assert_eq!(identifier("x]; DROP DATABASE y; --"), "[x]]; DROP DATABASE y; --]");
    }

    #[test]
    fn literal_doubles_quotes() {
        assert_eq!(literal("O'Brien's"), "O''Brien''s");
    }

    #[test]
    fn escape_like_takes_wildcards_as_themselves() {
        assert_eq!(escape_like("dnn_dev%[x]"), "dnn[_]dev[%][[]x]");
    }

    #[test]
    fn a_qualifier_is_letters_digits_and_underscores() {
        assert!(dnn_tables::is_qualifier(""));
        assert!(dnn_tables::is_qualifier("dnn_"));
        assert!(!dnn_tables::is_qualifier("x]; DROP TABLE y; --"));
        assert!(!dnn_tables::is_qualifier("dnn-"));
        assert_eq!(dnn_tables::name("dnn_", "Users"), "dbo.[dnn_Users]");
    }
}
