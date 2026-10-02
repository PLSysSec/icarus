//! C++ names to Cachet identifiers, decided once per unit.
//!
//! Two things force a name to change. A C++ name can be a Cachet keyword --
//! `JSOpToCondition` takes a parameter called `op` -- and the mangled name it gets
//! instead has to be one nothing else in the unit is using, or the rename would
//! quietly capture. Both are properties of the whole unit, not of one statement, so
//! the mapping is computed up front and then only looked up.

use std::collections::{HashMap, HashSet};

use cachet_lang::ast::Ident;

use crate::cpp_subset::{CompoundStmt, Param, Stmt, Visit, walk_stmt};

/// Names Cachet will not accept as an identifier.
///
/// Measured against `cachet-compiler` in both parameter and `let` position: every
/// name here but `left` and `right` is refused in both. Those two are accepted in
/// both, so they are listed only out of caution -- other positions were not
/// probed, and being over-broad costs a gratuitous rename where being under-broad
/// costs a parse error.
const RESERVED: &[&str] = &[
    "as", "asc", "assert", "assume", "bind", "desc", "else", "emit", "emits", "enum", "fn", "for",
    "goto", "if", "impl", "import", "in", "ir", "label", "left", "let", "mut", "op", "out",
    "return", "right", "struct", "unreachable", "unsafe", "var",
];

pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// What each C++ name in one unit is called in Cachet.
///
/// Consulted at declarations *and* at references, so the two cannot disagree.
#[derive(Debug, Default)]
pub struct NameMap {
    /// C++ name to the Cachet identifier standing for it.
    map: HashMap<String, Ident>,
    /// Every identifier handed out, so a [`NameMap::claim`] can steer clear.
    taken: HashSet<String>,
}

impl NameMap {
    /// Decides a name for every C++ name the unit declares.
    ///
    /// An acceptable name maps to itself, which keeps C++'s own shadowing intact:
    /// two scopes declaring `reg` both get `reg`, and Cachet shadows the same way.
    /// A reserved name grows `_` until the result is acceptable, unclaimed by
    /// another C++ name, and not already handed to some other reserved one.
    pub fn build<'a>(declared: impl IntoIterator<Item = &'a str>) -> NameMap {
        let declared: Vec<&str> = declared.into_iter().collect();
        // Every raw name counts as claimed: a name that is not reserved maps to
        // itself, so mangling onto it would collide.
        let claimed: HashSet<&str> = declared.iter().copied().collect();
        let mut names = NameMap::default();

        for name in declared {
            if names.map.contains_key(name) {
                continue;
            }
            let mut cachet = name.to_owned();
            while is_reserved(&cachet)
                || (cachet != name
                    && (claimed.contains(cachet.as_str()) || names.taken.contains(&cachet)))
            {
                cachet.push('_');
            }
            names.taken.insert(cachet.clone());
            names.map.insert(name.to_owned(), Ident::from(cachet));
        }
        names
    }

    /// A name for something the unit does not declare, kept clear of everything it
    /// does.
    ///
    /// An op's field operand is the one name phoenix invents: it comes from the
    /// yaml, not from the C++, because the C++ has only an offset there. So it is
    /// the one name that can land on a local by accident -- a local called `offset`
    /// would shadow a field operand called `offset`, and the load that follows would
    /// quietly read the local.
    pub fn claim(&mut self, wanted: &str) -> Ident {
        let mut name = wanted.to_owned();
        while is_reserved(&name) || self.taken.contains(&name) {
            name.push('_');
        }
        self.taken.insert(name.clone());
        Ident::from(name)
    }

    /// The Cachet identifier for a C++ name.
    ///
    /// A name the unit never declared -- a field, a global -- still has to be
    /// acceptable, so it is mangled on its own. Without the unit's other names to
    /// check against this cannot promise freedom from collision, which is why every
    /// declaration goes through [`NameMap::build`] instead.
    pub fn ident(&self, cpp: &str) -> Ident {
        match self.map.get(cpp) {
            Some(ident) => ident.clone(),
            None if is_reserved(cpp) => Ident::from(format!("{cpp}_")),
            None => Ident::from(cpp.to_owned()),
        }
    }
}

/// Every name a unit declares: its parameters, and each local and label in its
/// body.
pub fn declared_names(params: &[Param], body: &CompoundStmt) -> Vec<String> {
    let mut collector = Declared(Vec::new());
    collector.visit_block(body);
    params
        .iter()
        .map(|param| param.name.clone())
        .chain(collector.0)
        .collect()
}

struct Declared(Vec<String>);

impl Visit for Declared {
    fn visit_stmt(&mut self, s: &Stmt) {
        // A `Label` declaration is a `Let` like any other in the subset, so both
        // kinds of binding are caught here.
        if let Stmt::Let(l) = s {
            self.0.push(l.name.clone());
        }
        walk_stmt(self, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyword_is_mangled() {
        let names = NameMap::build(["op", "isSigned"]);
        assert_eq!(names.ident("op").to_string(), "op_");
        assert_eq!(names.ident("isSigned").to_string(), "isSigned");
    }

    /// ```text
    /// bool f(JSOp op) { int op_ = 1; return true; }
    /// ```
    ///
    /// Mangling `op` onto `op_` would shadow the local, and Cachet accepts
    /// shadowing silently, so a later reference to the parameter would bind to the
    /// local instead.
    #[test]
    fn mangling_does_not_land_on_a_name_already_in_use() {
        let names = NameMap::build(["op", "op_"]);
        assert_eq!(names.ident("op_").to_string(), "op_");
        assert_eq!(names.ident("op").to_string(), "op__");
    }

    /// Both are reserved, and the first to claim `in_` keeps it.
    #[test]
    fn two_keywords_do_not_collide_with_each_other() {
        let names = NameMap::build(["in", "in_"]);
        assert_eq!(names.ident("in_").to_string(), "in_");
        assert_eq!(names.ident("in").to_string(), "in__");
    }

    /// A name declared in two sibling scopes is one C++ name and stays one Cachet
    /// name, so C++'s own shadowing carries over unchanged.
    #[test]
    fn a_repeated_declaration_keeps_one_name() {
        let names = NameMap::build(["reg", "reg"]);
        assert_eq!(names.ident("reg").to_string(), "reg");
    }

    /// An op's field operand is named by the yaml, so it can want a name the C++
    /// body is already using. A local called `offset` would shadow it, and the
    /// `emitLoadStubField` after that local would read the local.
    #[test]
    fn a_claim_steers_clear_of_a_declared_name() {
        let mut names = NameMap::build(["offset", "reg"]);
        assert_eq!(names.claim("offset").to_string(), "offset_");
        assert_eq!(names.claim("index").to_string(), "index");
    }

    #[test]
    fn a_claim_steers_clear_of_a_keyword() {
        let mut names = NameMap::build(["reg"]);
        assert_eq!(names.claim("op").to_string(), "op_");
    }

    /// Two field operands of one op could in principle want the same name.
    #[test]
    fn two_claims_of_one_name_differ() {
        let mut names = NameMap::build([]);
        assert_eq!(names.claim("val").to_string(), "val");
        assert_eq!(names.claim("val").to_string(), "val_");
    }

    /// A field or global the unit never declared still has to parse.
    #[test]
    fn an_undeclared_name_is_still_made_acceptable() {
        let names = NameMap::build(["unrelated"]);
        assert_eq!(names.ident("label").to_string(), "label_");
        assert_eq!(names.ident("op_").to_string(), "op_");
    }
}
