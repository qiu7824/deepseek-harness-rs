//! Rust port of the TS `reserved.spec.ts` suite for `dsh-code-runtime`: the
//! seam-owned portable-identifier exclusion sets every backend imports
//! rather than re-declares.

use dsh_code_runtime::{
    is_dunder_member, portable_reserved_words, reserved_binding_globals, reserved_error_members,
};

#[test]
fn reserved_binding_globals_covers_each_backend_owned_slot() {
    for name in ["console", "__dsh_main__", "__builtins__", "__name__", "__debug__"] {
        assert!(reserved_binding_globals().contains(name), "{name} reserved");
    }
    assert!(!reserved_binding_globals().contains("tools"));
}

#[test]
fn reserved_error_members_covers_the_js_error_and_python_exception_protocol_members() {
    for name in ["name", "message", "stack", "args", "with_traceback", "add_note"] {
        assert!(reserved_error_members().contains(name), "{name} reserved");
    }
    assert!(!reserved_error_members().contains("code"));
}

#[test]
fn dunder_member_matches_dunder_form_names_only() {
    assert!(is_dunder_member("__dict__"));
    assert!(is_dunder_member("__init__"));
    assert!(!is_dunder_member("_private"));
    assert!(!is_dunder_member("name"));
    assert!(!is_dunder_member("__mid"));
    // `__` has an empty middle — not a real CPython dunder.
    assert!(!is_dunder_member("__"));
    // `____` also has an empty middle between the two `__` pairs.
    assert!(!is_dunder_member("____"));
    // A single character between the pairs is the shortest real dunder form.
    assert!(is_dunder_member("__x__"));
}

#[test]
fn portable_reserved_words_is_the_union_of_ecmascript_and_python_reserved_words() {
    // ECMAScript-only keyword.
    assert!(portable_reserved_words().contains("function"));
    // Python-only keyword — refused here so the list stays portable.
    assert!(portable_reserved_words().contains("lambda"));
    assert!(portable_reserved_words().contains("nonlocal"));
    // Shared keyword.
    assert!(portable_reserved_words().contains("class"));
    // Ordinary identifier is not reserved.
    assert!(!portable_reserved_words().contains("tools"));
}
