//! Tree-sitter based usage detection.
//!
//! Replaces the old regex pile with a single parse-once-per-file approach:
//!
//!   1. Parse the file with the appropriate grammar.
//!   2. Walk the AST, collecting every identifier that is a *usage*
//!      (not a declaration, not inside a comment or string literal).
//!   3. While walking, collect import alias mappings
//!      (e.g. `import { InvoiceService as BillingManager }` → BillingManager→InvoiceService).
//!   4. Expand the used-identifier set through those aliases before returning.
//!
//! Callers receive a `HashSet<String>` of canonical names that are observably
//! used in the file.  A single O(1) `.contains()` per concept replaces the
//! entire old regex tree.

use std::collections::{HashMap, HashSet};
use tree_sitter::{Language, Node, Parser};

pub enum SupportedLanguage {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    CSharp,
    Ruby,
}

impl SupportedLanguage {
    pub fn from_file_path(path: &str) -> Option<Self> {
        if path.ends_with(".rs") {
            Some(SupportedLanguage::Rust)
        } else if path.ends_with(".py") {
            Some(SupportedLanguage::Python)
        } else if path.ends_with(".ts") || path.ends_with(".tsx") {
            Some(SupportedLanguage::TypeScript)
        } else if path.ends_with(".js") || path.ends_with(".jsx") {
            Some(SupportedLanguage::JavaScript)
        } else if path.ends_with(".go") {
            Some(SupportedLanguage::Go)
        } else if path.ends_with(".java") {
            Some(SupportedLanguage::Java)
        } else if path.ends_with(".cs") {
            Some(SupportedLanguage::CSharp)
        } else if path.ends_with(".rb") {
            Some(SupportedLanguage::Ruby)
        } else {
            None
        }
    }

    pub fn tree_sitter_language(&self) -> Language {
        match self {
            SupportedLanguage::Rust       => tree_sitter_rust::language(),
            SupportedLanguage::Python     => tree_sitter_python::language(),
            SupportedLanguage::TypeScript => tree_sitter_typescript::language_typescript(),
            SupportedLanguage::JavaScript => tree_sitter_javascript::language(),
            SupportedLanguage::Go         => tree_sitter_go::language(),
            SupportedLanguage::Java       => tree_sitter_java::language(),
            SupportedLanguage::CSharp     => tree_sitter_c_sharp::language(),
            SupportedLanguage::Ruby       => tree_sitter_ruby::language(),
        }
    }
}

pub struct TreeSitterUsageDetector {
    parser: Parser,
}

impl Default for TreeSitterUsageDetector {
    fn default() -> Self { Self::new() }
}

impl TreeSitterUsageDetector {
    pub fn new() -> Self {
        Self { parser: Parser::new() }
    }

    /// Parse `text` once and return the set of canonical concept names that
    /// are observably *used* in the file (not declared, not in
    /// comments/strings).  Import aliases are resolved so that
    ///
    ///   import { InvoiceService as BillingManager } from './billing';
    ///   const c = new BillingManager();
    ///
    /// correctly reports "InvoiceService" as used, not just "BillingManager".
    pub fn extract_used_identifiers(
        &mut self,
        text: &str,
        file_rel: &str,
    ) -> HashSet<String> {
        let lang = match SupportedLanguage::from_file_path(file_rel) {
            Some(l) => l,
            None    => return HashSet::new(),
        };

        if self.parser.set_language(lang.tree_sitter_language()).is_err() {
            return HashSet::new();
        }

        let tree = match self.parser.parse(text, None) {
            Some(t) => t,
            None    => return HashSet::new(),
        };

        let bytes = text.as_bytes();
        let root  = tree.root_node();

        // ── Pass 1: collect import aliases ───────────────────────────────────
        // alias → canonical  e.g. "BillingManager" → "InvoiceService"
        let mut aliases: HashMap<String, String> = HashMap::new();
        Self::collect_aliases(&root, bytes, &lang, &mut aliases);

        // ── Pass 2: collect all non-declaration identifiers ──────────────────
        let mut raw: HashSet<String> = HashSet::new();
        Self::collect_identifiers(&root, bytes, &lang, &mut raw);

        // ── Pass 3: expand aliases → canonical names ─────────────────────────
        // If a file uses "BillingManager" and the alias map says
        // BillingManager→InvoiceService, insert InvoiceService into the set
        // so the concept lookup hits.
        // Also insert the PascalCase variant of each identifier so that
        // ORM/client accessor patterns like `db.widget.findMany()` (which
        // produce the token "widget") still match the concept "Widget".
        let mut out: HashSet<String> = HashSet::new();
        for name in &raw {
            if let Some(canonical) = aliases.get(name.as_str()) {
                out.insert(canonical.clone());
            }
            // PascalCase variant: "widget" → "Widget", "invoiceService" → "InvoiceService"
            let pascal = {
                let mut chars = name.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            };
            if !pascal.is_empty() && pascal != *name {
                out.insert(pascal);
            }
            out.insert(name.clone());
        }
        out
    }

    // ── alias collection ─────────────────────────────────────────────────────

    fn collect_aliases(node: &Node, bytes: &[u8], lang: &SupportedLanguage, out: &mut HashMap<String, String>) {
        match lang {
            SupportedLanguage::TypeScript | SupportedLanguage::JavaScript => {
                // import { InvoiceService as BillingManager } from '...'
                // AST: import_specifier → name: "InvoiceService", alias: "BillingManager"
                if node.kind() == "import_specifier" {
                    let name  = node.child_by_field_name("name");
                    let alias = node.child_by_field_name("alias");
                    if let (Some(n), Some(a)) = (name, alias) {
                        let canonical = node_text(n, bytes);
                        let local     = node_text(a, bytes);
                        if !canonical.is_empty() && !local.is_empty() && canonical != local {
                            out.insert(local, canonical);
                        }
                    }
                }
            }
            SupportedLanguage::Python => {
                // from services.billing import InvoiceService as BillingManager
                // AST: aliased_import → name: "InvoiceService", alias: "BillingManager"
                if node.kind() == "aliased_import" {
                    let name  = node.child_by_field_name("name");
                    let alias = node.child_by_field_name("alias");
                    if let (Some(n), Some(a)) = (name, alias) {
                        let canonical = node_text(n, bytes);
                        let local     = node_text(a, bytes);
                        if !canonical.is_empty() && !local.is_empty() && canonical != local {
                            out.insert(local, canonical);
                        }
                    }
                }
            }
            SupportedLanguage::Rust => {
                // use crate::model::Index as Idx;
                // AST: use_as_clause → name: "Index", alias: "Idx"
                if node.kind() == "use_as_clause" {
                    // tree-sitter-rust represents this as:
                    //   (use_as_clause path: ... name: (identifier) "as" (identifier))
                    // The two identifiers are the second-to-last and last children.
                    let mut children: Vec<Node> = Vec::new();
                    let mut c = node.walk();
                    if c.goto_first_child() {
                        loop {
                            let ch = c.node();
                            if ch.kind() == "identifier" { children.push(ch); }
                            if !c.goto_next_sibling() { break; }
                        }
                    }
                    if children.len() >= 2 {
                        let canonical = node_text(children[children.len() - 2], bytes);
                        let local     = node_text(children[children.len() - 1], bytes);
                        if !canonical.is_empty() && !local.is_empty() && canonical != local {
                            out.insert(local, canonical);
                        }
                    }
                }
            }
            // Go and Java don't have rename-import syntax in common use
            SupportedLanguage::Go | SupportedLanguage::Java => {}
            // C#: using BillingManager = Services.InvoiceService;
            SupportedLanguage::CSharp => {
                if node.kind() == "using_directive" {
                    // tree-sitter-c-sharp: (using_directive (name_equals (identifier) "=") qualified_name)
                    let text = std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()]).unwrap_or("");
                    if let Some(eq_pos) = text.find('=') {
                        let alias = text[..eq_pos].trim()
                            .trim_start_matches("using").trim().to_string();
                        let canonical = text[eq_pos + 1..].trim()
                            .trim_end_matches(';').trim()
                            .split('.').last().unwrap_or("").to_string();
                        if !alias.is_empty() && !canonical.is_empty() && alias != canonical {
                            out.insert(alias, canonical);
                        }
                    }
                }
            }
            // Ruby doesn't have standard import aliasing at the language level
            SupportedLanguage::Ruby => {}
        }

        // Recurse
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                Self::collect_aliases(&cursor.node(), bytes, lang, out);
                if !cursor.goto_next_sibling() { break; }
            }
        }
    }

    // ── identifier collection ─────────────────────────────────────────────────

    fn collect_identifiers(
        node: &Node,
        bytes: &[u8],
        lang: &SupportedLanguage,
        out: &mut HashSet<String>,
    ) {
        let kind = node.kind();

        // Skip comments and strings — no false positives from literals
        if kind.contains("comment") || kind.contains("string") {
            return;
        }

        if kind == "identifier" || kind == "type_identifier" || kind == "field_identifier" || kind == "property_identifier" {
            let name = node_text(*node, bytes);
            if !name.is_empty() && !Self::is_declaration(node, lang) {
                out.insert(name);
            }
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                Self::collect_identifiers(&cursor.node(), bytes, lang, out);
                if !cursor.goto_next_sibling() { break; }
            }
        }
    }

    // ── declaration filter ────────────────────────────────────────────────────

    /// Returns true if `node` is the *name* of a declaration (struct, class,
    /// fn, etc.) — i.e. it is defining the concept, not referencing it.
    fn is_declaration(node: &Node, lang: &SupportedLanguage) -> bool {
        let parent = match node.parent() {
            Some(p) => p,
            None    => return false,
        };
        let is_name_field = parent.child_by_field_name("name").map(|n| n.id()) == Some(node.id());

        match lang {
            SupportedLanguage::Rust => matches!(
                parent.kind(),
                "struct_item" | "enum_item" | "trait_item" | "type_item" | "function_item"
            ) && is_name_field,

            SupportedLanguage::Python => matches!(
                parent.kind(),
                "class_definition" | "function_definition"
            ) && is_name_field,

            SupportedLanguage::TypeScript | SupportedLanguage::JavaScript => matches!(
                parent.kind(),
                "class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "function_declaration"
                | "method_definition"
            ) && is_name_field,

            SupportedLanguage::Go => matches!(
                parent.kind(),
                "type_spec" | "function_declaration" | "method_declaration"
            ) && is_name_field,

            SupportedLanguage::Java => matches!(
                parent.kind(),
                "class_declaration" | "interface_declaration" | "enum_declaration" | "method_declaration"
            ) && is_name_field,

            SupportedLanguage::CSharp => matches!(
                parent.kind(),
                "class_declaration" | "interface_declaration" | "struct_declaration"
                | "enum_declaration" | "method_declaration" | "record_declaration"
            ) && is_name_field,

            SupportedLanguage::Ruby => matches!(
                parent.kind(),
                "class" | "module" | "method" | "singleton_method"
            ) && is_name_field,
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn node_text(node: Node, bytes: &[u8]) -> String {
    std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()])
        .unwrap_or("")
        .to_string()
}
