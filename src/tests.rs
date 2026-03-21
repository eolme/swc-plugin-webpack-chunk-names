use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{ExprOrSpread, Ident, TplElement};
use swc_core::ecma::transforms::testing::{test_inline, Tester};
use swc_core::ecma::visit::visit_mut_pass;

use super::*;

fn make_tpl(quasis_strs: &[&str], expr_names: &[&str]) -> Tpl {
    let quasis = quasis_strs
        .iter()
        .enumerate()
        .map(|(i, s)| TplElement {
            span: DUMMY_SP,
            tail: i == quasis_strs.len() - 1,
            cooked: Some((*s).into()),
            raw: Atom::from(*s),
        })
        .collect();
    let exprs = expr_names
        .iter()
        .map(|name| {
            Box::new(Expr::Ident(Ident {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                sym: Atom::from(*name),
                optional: false,
            })) as Box<Expr>
        })
        .collect();
    Tpl { span: DUMMY_SP, quasis, exprs }
}

fn source_dirs() -> Option<&'static [String]> {
    None
}

// --- to_camel_case ---

#[test]
fn camel_case_kebab() {
    assert_eq!(to_camel_case("package-name"), "packageName");
}

#[test]
fn camel_case_snake() {
    assert_eq!(to_camel_case("package_name"), "packageName");
}

#[test]
fn camel_case_mixed() {
    assert_eq!(to_camel_case("my-cool_lib"), "myCoolLib");
}

#[test]
fn camel_case_no_separators() {
    assert_eq!(to_camel_case("lodash"), "lodash");
}

// --- strip_extension ---

#[test]
fn strip_ts_extension() {
    assert_eq!(strip_extension("Component.tsx"), "Component");
    assert_eq!(strip_extension("utils.ts"), "utils");
    assert_eq!(strip_extension("index.js"), "index");
    assert_eq!(strip_extension("helper.mjs"), "helper");
}

#[test]
fn strip_no_extension() {
    assert_eq!(strip_extension("ComponentName"), "ComponentName");
}

#[test]
fn strip_any_extension() {
    assert_eq!(strip_extension("data.json"), "data");
    assert_eq!(strip_extension("style.css"), "style");
    assert_eq!(strip_extension("template.vue"), "template");
}

#[test]
fn strip_hidden_file_unchanged() {
    assert_eq!(strip_extension(".hidden"), ".hidden");
}

// --- resolve_relative_path ---

#[test]
fn resolve_relative_current_dir() {
    let resolved = resolve_relative_path("/project/src/components/App.tsx", "./Button");
    assert_eq!(resolved, "project/src/components/Button");
}

#[test]
fn resolve_relative_parent_dir() {
    let resolved = resolve_relative_path("/project/src/components/App.tsx", "../utils/helpers");
    assert_eq!(resolved, "project/src/utils/helpers");
}

#[test]
fn resolve_deeply_nested() {
    let resolved = resolve_relative_path("/project/src/deep/nested/File.tsx", "../../other/Thing");
    assert_eq!(resolved, "project/src/other/Thing");
}

// --- extract_package_from_path ---

#[test]
fn extract_package_before_src() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("project/mf-widgets/src/components/Button", dirs);
    assert_eq!(pkg, Some("mf-widgets"));
}

#[test]
fn extract_package_before_lib() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("packages/my-lib/lib/utils", dirs);
    assert_eq!(pkg, Some("my-lib"));
}

#[test]
fn extract_package_fallback() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("foo/bar/baz", dirs);
    assert_eq!(pkg, Some("bar"));
}

#[test]
fn extract_package_right_to_left() {
    let dirs = source_dirs();
    let pkg =
        extract_package_from_path("project/src/packages/mf-widgets/src/components/Button", dirs);
    assert_eq!(pkg, Some("mf-widgets"));
}

// --- generate_chunk_name ---

#[test]
fn chunk_name_scoped_package() {
    let dirs = source_dirs();
    let name =
        generate_chunk_name("@my-company/package-name/src/my/test/ComponentName", None, dirs);
    assert_eq!(name, Some("myCompanyPackageName.ComponentName".to_string()));
}

#[test]
fn chunk_name_scoped_with_extension() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@scope/my-lib/utils/helpers.ts", None, dirs);
    assert_eq!(name, Some("scopeMyLib.helpers".to_string()));
}

#[test]
fn chunk_name_scoped_distinct_scopes() {
    let dirs = source_dirs();
    let scope_a = generate_chunk_name("@scope-a/ui/Button.tsx", None, dirs);
    let scope_b = generate_chunk_name("@scope-b/ui/Button.tsx", None, dirs);
    assert_eq!(scope_a, Some("scopeAUi.Button".to_string()));
    assert_eq!(scope_b, Some("scopeBUi.Button".to_string()));
    assert_ne!(scope_a, scope_b);
}

#[test]
fn chunk_name_non_scoped() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash/fp/get", None, dirs);
    assert_eq!(name, Some("lodash.get".to_string()));
}

#[test]
fn chunk_name_dotted_bare_and_deep_import_collide() {
    let dirs = source_dirs();
    let dotted = generate_chunk_name("lodash.get", None, dirs);
    let deep = generate_chunk_name("lodash/get", None, dirs);
    assert_eq!(dotted, Some("lodash.get".to_string()));
    assert_eq!(deep, Some("lodash.get".to_string()));
    assert_eq!(dotted, deep);
}

#[test]
fn chunk_name_bare_deep_path_with_source_dir() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lib/lib/test-package/src/Component", None, dirs);
    assert_eq!(name, Some("testPackage.Component".to_string()));
}

#[test]
fn chunk_name_relative_with_filename() {
    let dirs = source_dirs();
    let name =
        generate_chunk_name("./components/MyWidget", Some("/project/mf-widgets/src/App.tsx"), dirs);
    assert_eq!(name, Some("mfWidgets.MyWidget".to_string()));
}

#[test]
fn chunk_name_relative_no_filename() {
    let dirs = source_dirs();
    let name = generate_chunk_name("./components/MyWidget", None, dirs);
    assert_eq!(name, None);
}

#[test]
fn chunk_name_relative_with_extension() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "./Button.tsx",
        Some("/project/mf-widgets/src/components/App.tsx"),
        dirs,
    );
    assert_eq!(name, Some("mfWidgets.Button".to_string()));
}

#[test]
fn chunk_name_relative_filename_with_cwd_matches_absolute() {
    let dirs = source_dirs();
    let abs =
        generate_chunk_name("./components/MyWidget", Some("/project/mf-widgets/src/App.tsx"), dirs);
    let rel_with_cwd =
        generate_chunk_name("./components/MyWidget", Some("/project/mf-widgets/src/App.tsx"), dirs);
    assert_eq!(abs, rel_with_cwd);

    let cwd_joined = {
        let c = "/project";
        let f = "mf-widgets/src/App.tsx";
        let mut s = String::with_capacity(c.len() + 1 + f.len());
        s.push_str(c);
        s.push('/');
        s.push_str(f);
        s
    };
    let from_joined = generate_chunk_name("./components/MyWidget", Some(&cwd_joined), dirs);
    assert_eq!(abs, from_joined);
}

#[test]
fn chunk_name_relative_filename_cwd_trailing_slash() {
    let dirs = source_dirs();
    let c = "/project/";
    let f = "mf-widgets/src/App.tsx";
    let c_trimmed = c.strip_suffix('/').unwrap_or(c);
    let mut joined = String::with_capacity(c_trimmed.len() + 1 + f.len());
    joined.push_str(c_trimmed);
    joined.push('/');
    joined.push_str(f);

    let name = generate_chunk_name("./components/MyWidget", Some(&joined), dirs);
    assert_eq!(name, Some("mfWidgets.MyWidget".to_string()));
}

#[test]
fn chunk_name_absolute_leading_slash_returns_none() {
    let dirs = source_dirs();
    let name = generate_chunk_name("/project/mf-widgets/src/components/Button.tsx", None, dirs);
    assert_eq!(name, None);
}

// --- parse_plugin_config ---

#[test]
fn parse_plugin_config_missing_uses_defaults() {
    assert_eq!(parse_plugin_config(None), None);
}

#[test]
fn parse_plugin_config_invalid_uses_defaults() {
    assert_eq!(parse_plugin_config(Some("{invalid".to_string())), None);
}

#[test]
fn parse_plugin_config_valid_uses_source_dirs() {
    assert_eq!(
        parse_plugin_config(Some(r#"{"dirs":["client","shared"]}"#.to_string())),
        Some(vec!["client".to_string(), "shared".to_string()].into_boxed_slice())
    );
}

// --- is_fallback_import_call ---

fn make_ident_expr(name: &str) -> Box<Expr> {
    Box::new(Expr::Ident(Ident {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        sym: Atom::from(name),
        optional: false,
    }))
}

#[test]
fn fallback_import_ident_call_is_supported() {
    let call = CallExpr {
        span: DUMMY_SP,
        callee: swc_core::ecma::ast::Callee::Expr(make_ident_expr("import")),
        args: vec![ExprOrSpread {
            spread: None,
            expr: Box::new(Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
                span: DUMMY_SP,
                value: "./module".into(),
                raw: None,
            }))),
        }],
        type_args: None,
        ctxt: SyntaxContext::empty(),
    };
    assert!(is_fallback_import_call(&call));
}

#[test]
fn non_import_ident_call_is_not_fallback_import() {
    let call = CallExpr {
        span: DUMMY_SP,
        callee: swc_core::ecma::ast::Callee::Expr(make_ident_expr("load")),
        args: vec![],
        type_args: None,
        ctxt: SyntaxContext::empty(),
    };
    assert!(!is_fallback_import_call(&call));
}

// --- flatten_tpl ---

#[test]
fn flatten_tpl_single_expr() {
    let tpl = make_tpl(&["./locale/", ""], &["language"]);
    assert_eq!(flatten_tpl(&tpl), Some("./locale/dynamic".to_string()));
}

#[test]
fn flatten_tpl_multiple_exprs() {
    let tpl = make_tpl(&["./", "/", ""], &["scope", "name"]);
    assert_eq!(flatten_tpl(&tpl), Some("./dynamic/dynamic".to_string()));
}

#[test]
fn flatten_tpl_no_exprs() {
    let tpl = make_tpl(&["./static/path"], &[]);
    assert_eq!(flatten_tpl(&tpl), Some("./static/path".to_string()));
}

#[test]
fn flatten_tpl_generates_chunk_name() {
    let dirs = source_dirs();
    let path = flatten_tpl(&make_tpl(&["./locale/", ""], &["language"])).unwrap();
    let name = generate_chunk_name(&path, Some("/project/mf-widgets/src/App.tsx"), dirs);
    assert_eq!(name, Some("mfWidgets.dynamic".to_string()));
}

// --- generate_chunk_name: bare packages ---

#[test]
fn chunk_name_bare_non_scoped() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash", None, dirs);
    assert_eq!(name, Some("lodash".to_string()));
}

#[test]
fn chunk_name_bare_non_scoped_with_separator() {
    let dirs = source_dirs();
    let name = generate_chunk_name("my-package", None, dirs);
    assert_eq!(name, Some("myPackage".to_string()));
}

#[test]
fn chunk_name_bare_scoped() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@scope/pkg", None, dirs);
    assert_eq!(name, Some("scopePkg".to_string()));
}

#[test]
fn chunk_name_bare_scoped_with_separators() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@my-scope/my-pkg", None, dirs);
    assert_eq!(name, Some("myScopeMyPkg".to_string()));
}

#[test]
fn chunk_name_scoped_trailing_slash_returns_none() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@scope/", None, dirs);
    assert_eq!(name, None);
}

#[test]
fn chunk_name_empty_returns_none() {
    let dirs = source_dirs();
    let name = generate_chunk_name("", None, dirs);
    assert_eq!(name, None);
}

// --- to_camel_case: edge cases ---

#[test]
fn camel_case_leading_separator() {
    assert_eq!(to_camel_case("-foo"), "Foo");
}

#[test]
fn camel_case_trailing_separator() {
    assert_eq!(to_camel_case("foo-"), "foo");
}

#[test]
fn camel_case_consecutive_separators() {
    assert_eq!(to_camel_case("a--b"), "aB");
}

#[test]
fn camel_case_empty() {
    assert_eq!(to_camel_case(""), "");
}

// --- strip_extension: edge cases ---

#[test]
fn strip_double_extension() {
    assert_eq!(strip_extension("file.test.tsx"), "file.test");
}

#[test]
fn strip_extension_dot_only() {
    assert_eq!(strip_extension("."), ".");
}

#[test]
fn strip_extension_empty() {
    assert_eq!(strip_extension(""), "");
}

// --- resolve_relative_path: edge cases ---

#[test]
fn resolve_relative_bare_filename() {
    let resolved = resolve_relative_path("App.tsx", "./components/Button");
    assert_eq!(resolved, "components/Button");
}

#[test]
fn resolve_relative_excessive_parent_traversal() {
    let resolved = resolve_relative_path("/a/b/File.tsx", "../../../x");
    assert_eq!(resolved, "x");
}

// --- extract_package_from_path: edge cases ---

#[test]
fn extract_package_single_segment() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("onlyone", dirs);
    assert_eq!(pkg, Some("onlyone"));
}

#[test]
fn extract_package_empty_path() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("", dirs);
    assert_eq!(pkg, None);
}

// --- flatten_tpl: edge cases ---

#[test]
fn flatten_tpl_empty_quasis() {
    let tpl = make_tpl(&[""], &[]);
    assert_eq!(flatten_tpl(&tpl), None);
}

#[test]
fn flatten_tpl_only_expr() {
    let tpl = make_tpl(&["", ""], &["name"]);
    assert_eq!(flatten_tpl(&tpl), Some("dynamic".to_string()));
}

// --- generate_chunk_name: nested paths ---

#[test]
fn chunk_name_nested_components_via_lib() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "./components/GroupName",
        Some("/project/my-app/lib/components/Group/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("myApp.GroupName".to_string()));
}

#[test]
fn chunk_name_nested_components_via_src() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "./components/GroupName",
        Some("/project/mf-widgets/src/components/Group/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("mfWidgets.GroupName".to_string()));
}

#[test]
fn chunk_name_nested_parent_traversal_into_sibling_group() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "../GroupB/components/GroupBChild",
        Some("/project/mf-widgets/src/components/GroupA/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("mfWidgets.GroupBChild".to_string()));
}

#[test]
fn chunk_name_deeply_nested_upward_traversal() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "../../GroupTitle",
        Some("/project/mf-widgets/src/components/Group/components/GroupName/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("mfWidgets.GroupTitle".to_string()));
}

#[test]
fn extract_package_nested_components_finds_innermost_source_dir() {
    let dirs = source_dirs();
    let pkg =
        extract_package_from_path("project/my-app/lib/components/Group/components/GroupName", dirs);
    assert_eq!(pkg, Some("my-app"));
}

// --- generate_chunk_name: node_modules ---

#[test]
fn extract_package_nested_node_modules_with_src() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path(
        "node_modules/@scope1/pkg1/node_modules/@scope2/pkg2/src/components/Button",
        dirs,
    );
    assert_eq!(pkg, Some("pkg2"));
}

#[test]
fn extract_package_single_node_modules_with_lib() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("node_modules/@scope/package/lib/utils/helper", dirs);
    assert_eq!(pkg, Some("package"));
}

#[test]
fn chunk_name_relative_import_inside_nested_node_modules() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "./components/Button",
        Some("/project/node_modules/@scope1/pkg1/node_modules/@scope2/pkg2/src/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("pkg2.Button".to_string()));
}

#[test]
fn chunk_name_relative_import_inside_single_node_modules() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "../utils/formatDate",
        Some("/project/node_modules/@company/ui-kit/src/components/DatePicker.tsx"),
        dirs,
    );
    assert_eq!(name, Some("uiKit.formatDate".to_string()));
}

#[test]
fn chunk_name_relative_import_unscoped_node_modules() {
    let dirs = source_dirs();
    let name = generate_chunk_name(
        "./locale/en",
        Some("/project/node_modules/some-lib/src/index.tsx"),
        dirs,
    );
    assert_eq!(name, Some("someLib.en".to_string()));
}

// --- generate_chunk_name: trailing slash ---

#[test]
fn chunk_name_non_scoped_trailing_slash() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash/", None, dirs);
    assert_eq!(name, Some("lodash.index".to_string()));
}

#[test]
fn chunk_name_non_scoped_deep_trailing_slash() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash/fp/", None, dirs);
    assert_eq!(name, Some("lodash.fp".to_string()));
}

#[test]
fn chunk_name_scoped_deep_trailing_slash() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@scope/pkg/sub/", None, dirs);
    assert_eq!(name, Some("scopePkg.sub".to_string()));
}

#[test]
fn chunk_name_relative_trailing_slash() {
    let dirs = source_dirs();
    let name = generate_chunk_name("./components/", Some("/project/mf-widgets/src/App.tsx"), dirs);
    assert_eq!(name, Some("mfWidgets.components".to_string()));
}

// --- generate_chunk_name: scoped edge cases ---

#[test]
fn chunk_name_at_sign_only_returns_none() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@scope", None, dirs);
    assert_eq!(name, None);
}

#[test]
fn chunk_name_at_alias_with_leaf() {
    let dirs = source_dirs();
    assert_eq!(
        generate_chunk_name("@/components/Button", None, dirs),
        Some("components.Button".to_string())
    );
}

#[test]
fn chunk_name_at_alias_deep_path() {
    let dirs = source_dirs();
    assert_eq!(
        generate_chunk_name("@/utils/helpers/format", None, dirs),
        Some("utils.format".to_string())
    );
}

#[test]
fn chunk_name_at_alias_bare() {
    let dirs = source_dirs();
    assert_eq!(generate_chunk_name("@/lib", None, dirs), Some("lib".to_string()));
}

#[test]
fn chunk_name_at_alias_trailing_slash() {
    let dirs = source_dirs();
    assert_eq!(generate_chunk_name("@/lib/", None, dirs), Some("lib.index".to_string()));
}

#[test]
fn chunk_name_non_scoped_with_index() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash/index", None, dirs);
    assert_eq!(name, Some("lodash.index".to_string()));
}

// --- generate_chunk_name: source-dir aware ---

#[test]
fn chunk_name_scoped_with_and_without_source_dir_same() {
    let dirs = source_dirs();
    let with_src =
        generate_chunk_name("@my-company/package-name/src/my/test/ComponentName", None, dirs);
    let without_src =
        generate_chunk_name("@my-company/package-name/my/test/ComponentName", None, dirs);
    assert_eq!(with_src, Some("myCompanyPackageName.ComponentName".to_string()));
    assert_eq!(with_src, without_src);
}

#[test]
fn chunk_name_at_alias_with_source_dir() {
    let dirs = source_dirs();
    let name = generate_chunk_name("@/lib/test-package/src/Component", None, dirs);
    assert_eq!(name, Some("testPackage.Component".to_string()));
}

#[test]
fn chunk_name_bare_without_source_dir_unchanged() {
    let dirs = source_dirs();
    let name = generate_chunk_name("lodash/fp/get", None, dirs);
    assert_eq!(name, Some("lodash.get".to_string()));
}

// --- extract_package_from_path: source-dir at root ---

#[test]
fn extract_package_source_dir_at_root_uses_fallback() {
    let dirs = source_dirs();
    let pkg = extract_package_from_path("src/components/Button", dirs);
    assert_eq!(pkg, Some("components"));
}

// --- to_camel_case: digits & degenerate ---

#[test]
fn camel_case_only_separators() {
    assert_eq!(to_camel_case("---"), "");
}

#[test]
fn camel_case_with_digits() {
    assert_eq!(to_camel_case("lib-2"), "lib2");
}

// --- flatten_tpl: non-ident expressions ---

#[test]
fn flatten_tpl_numeric_expr_uses_dynamic() {
    let tpl = Tpl {
        span: DUMMY_SP,
        quasis: vec![
            TplElement {
                span: DUMMY_SP,
                tail: false,
                cooked: Some("./v".into()),
                raw: Atom::from("./v"),
            },
            TplElement {
                span: DUMMY_SP,
                tail: true,
                cooked: Some("/api".into()),
                raw: Atom::from("/api"),
            },
        ],
        exprs: vec![Box::new(Expr::Lit(Lit::Num(swc_core::ecma::ast::Number {
            span: DUMMY_SP,
            value: 2.0,
            raw: None,
        })))],
    };
    assert_eq!(flatten_tpl(&tpl), Some("./vdynamic/api".to_string()));
}

#[test]
fn flatten_tpl_any_expr_uses_dynamic() {
    let tpl = Tpl {
        span: DUMMY_SP,
        quasis: vec![
            TplElement {
                span: DUMMY_SP,
                tail: false,
                cooked: Some("./path/".into()),
                raw: Atom::from("./path/"),
            },
            TplElement { span: DUMMY_SP, tail: true, cooked: Some("".into()), raw: Atom::from("") },
        ],
        exprs: vec![Box::new(Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
            span: DUMMY_SP,
            value: true,
        })))],
    };
    assert_eq!(flatten_tpl(&tpl), Some("./path/dynamic".to_string()));
}

// --- parse_plugin_config: edge cases ---

#[test]
fn parse_plugin_config_empty_object_uses_defaults() {
    assert_eq!(parse_plugin_config(Some("{}".to_string())), None);
}

#[test]
fn parse_plugin_config_extra_fields_ignored() {
    assert_eq!(
        parse_plugin_config(Some(r#"{"dirs":["client"],"unknown":true}"#.to_string())),
        Some(vec!["client".to_string()].into_boxed_slice())
    );
}

// --- generate_chunk_name: trailing-slash disambiguation ---

#[test]
fn chunk_name_punycode_builtin_vs_npm() {
    let dirs = source_dirs();
    let builtin = generate_chunk_name("punycode", None, dirs);
    let npm = generate_chunk_name("punycode/", None, dirs);
    assert_eq!(builtin, Some("punycode".to_string()));
    assert_eq!(npm, Some("punycode.index".to_string()));
    assert_ne!(builtin, npm);
}

#[test]
fn chunk_name_scoped_bare_vs_trailing_slash() {
    let dirs = source_dirs();
    let bare = generate_chunk_name("@scope/pkg", None, dirs);
    let with_slash = generate_chunk_name("@scope/pkg/", None, dirs);
    assert_eq!(bare, Some("scopePkg".to_string()));
    assert_eq!(with_slash, Some("scopePkg.index".to_string()));
    assert_ne!(bare, with_slash);
}

// --- test_inline (swc_ecma_transforms_testing via swc_core) ---

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_bare_import_injects_webpack_comments,
    r#"import("lodash/fp/get");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/fp/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_scoped_package_import,
    r#"import("@scope/pkg/sub/Leaf");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "scopePkg.Leaf" */"@scope/pkg/sub/Leaf");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/App.tsx".into()),
            source_dirs: None,
        })
    },
    inline_relative_import_with_filename,
    r#"import("./components/MyWidget");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "mfWidgets.MyWidget" */"./components/MyWidget");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_replaces_old_webpack_chunk_comments,
    r#"import(/* webpackChunkName: "old" */"lodash/fp");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_preserves_unrelated_leading_comments,
    r#"import(/* keep */ /* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/* keep *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_preserves_multiline_non_webpack_block_comment,
    r#"import(/*
note for readers
*/ /* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/*
note for readers
*//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_preserves_empty_block_comment_before_specifier,
    r#"import(/**/ /* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/**//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_preserves_multiline_block_with_blank_lines,
    r#"import(/*
 * licensing
 *
 * more text
 */ /* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/*
 * licensing
 *
 * more text
 *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_replaces_multiline_webpack_chunk_name_comment,
    r#"import(/* webpackChunkName:
"old" */"lodash/fp");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_replaces_multiline_webpack_mode_and_chunk_comments,
    r#"import(/*
webpackMode: "eager"
webpackChunkName: "gone"
*/"lodash");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_newline_between_kept_comment_and_webpack_chunk_comment,
    r#"import(/* keep */

/* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/* keep */

/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_newline_between_empty_block_and_webpack_chunk_comment,
    r#"import(/**/

/* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/**/

/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_double_newline_between_leading_comments,
    r#"import(/* note */


/* webpackChunkName: "old" */"lodash/get");"#,
    r#"import(/* note */


/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.get" */"lodash/get");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_newline_between_webpack_chunk_comment_and_specifier_string,
    r#"import(/* webpackChunkName: "old" */

"lodash/fp");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */

"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_newline_between_generated_webpack_magic_comments_in_output,
    r#"import("lodash/fp");"#,
    r#"import(/* webpackMode: "lazy-once" */

/* webpackChunkName: "lodash.fp" */"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_require_call_is_unchanged,
    r#"require("lodash");"#,
    r#"require("lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_absolute_specifier_skipped,
    r#"import("/absolute/module");"#,
    r#"import("/absolute/module");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_trailing_slash_index_chunk_name,
    r#"import("lodash/");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.index" */"lodash/");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_deep_path_trailing_slash_chunk_name,
    r#"import("lodash/fp/");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp/");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/App.tsx".into()),
            source_dirs: None,
        })
    },
    inline_relative_trailing_slash_index,
    r#"import("./components/");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "mfWidgets.components" */"./components/");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_static_template_literal_specifier,
    r#"import(`lodash/fp`);"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */`lodash/fp`);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_replaces_prior_webpack_mode_magic_comment,
    r#"import(/* webpackMode: "eager" */"lodash/fp");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_two_dynamic_imports_in_sequence,
    r#"import("lodash"); import("react");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash"); import(/* webpackMode: "lazy-once" *//* webpackChunkName: "react" */"react");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: Some(vec!["client".to_string()].into_boxed_slice()),
        })
    },
    inline_custom_config_source_dir_for_package_boundary,
    r#"import("acme/client/widgets/Card");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "acme.widgets.Card" */"acme/client/widgets/Card");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_variable_argument_is_unchanged,
    // A dynamic expression as the import argument cannot produce a chunk name.
    r#"import(modulePath);"#,
    r#"import(modulePath);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_member_expr_argument_is_unchanged,
    // Member-expression argument: obj.path – not a string/template, skip.
    r#"import(obj.path);"#,
    r#"import(obj.path);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_call_expr_argument_is_unchanged,
    // Call-expression argument: getPath() – not a string/template, skip.
    r#"import(getPath());"#,
    r#"import(getPath());"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_relative_import_without_filename_is_unchanged,
    // Relative specifier with no filename context → cannot resolve package → skip.
    r#"import("./components/Button");"#,
    r#"import("./components/Button");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_template_dynamic_bare_package,
    // Template literal whose expression(s) collapse to "dynamic" placeholder.
    r#"import(`lodash/${module}`);"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.dynamic" */`lodash/${module}`);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_template_dynamic_scoped_package,
    r#"import(`@scope/pkg/${sub}`);"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "scopePkg.dynamic" */`@scope/pkg/${sub}`);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/App.tsx".into()),
            source_dirs: None,
        })
    },
    inline_template_dynamic_relative_with_filename,
    // Relative template literal resolved via filename – expression becomes "dynamic".
    r#"import(`./locale/${language}`);"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "mfWidgets.dynamic" */`./locale/${language}`);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_template_dynamic_relative_without_filename_is_unchanged,
    // Relative template literal with no filename → cannot resolve → skip.
    r#"import(`./locale/${language}`);"#,
    r#"import(`./locale/${language}`);"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_at_alias_with_leaf,
    // @/ is treated as a bare-import alias whose scope segment is empty → package = first real segment.
    r#"import("@/components/Button");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "components.Button" */"@/components/Button");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_at_alias_bare,
    r#"import("@/lib");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lib" */"@/lib");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_at_alias_trailing_slash,
    r#"import("@/lib/");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lib.index" */"@/lib/");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_scoped_deep_trailing_slash,
    r#"import("@scope/pkg/sub/");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "scopePkg.sub" */"@scope/pkg/sub/");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_webpack_prefetch_comment_preserved,
    // webpackPrefetch is not a chunk-name/mode comment and must survive the transform.
    r#"import(/* webpackPrefetch: true */"lodash");"#,
    r#"import(/* webpackPrefetch: true *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_webpack_preload_comment_preserved,
    r#"import(/* webpackPreload: true */"lodash");"#,
    r#"import(/* webpackPreload: true *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_unknown_webpack_comment_preserved,
    // Generic unknown webpack magic comment must be kept verbatim.
    r#"import(/* webpackFetchPriority: "high" */"lodash");"#,
    r#"import(/* webpackFetchPriority: "high" *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_multiple_preserved_comments_with_replacement,
    // Several non-webpack block comments all survive; only the old chunk/mode ones are replaced.
    r#"import(/* a */ /* b */ /* webpackChunkName: "old" */"lodash/fp");"#,
    r#"import(/* a */ /* b *//* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_webpack_prefetch_and_old_chunk_name_replaced,
    // webpackPrefetch is preserved; stale webpackChunkName is replaced.
    r#"import(/* webpackPrefetch: true */ /* webpackChunkName: "stale" */"react");"#,
    r#"import(/* webpackPrefetch: true *//* webpackMode: "lazy-once" *//* webpackChunkName: "react" */"react");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_bare_single_segment_package,
    // Single-segment bare package name – no dot-suffix, just camelCase of the whole name.
    r#"import("lodash");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_bare_single_segment_hyphenated_package,
    r#"import("my-package");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "myPackage" */"my-package");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/App.tsx".into()),
            source_dirs: Some(vec!["client".to_string(), "shared".to_string()].into_boxed_slice()),
        })
    },
    inline_custom_source_dir_with_relative_import,
    // Custom dirs: "client" is a source boundary, so the package resolves correctly.
    r#"import("acme/client/widgets/Card");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "acme.widgets.Card" */"acme/client/widgets/Card");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/App.tsx".into()),
            source_dirs: None,
        })
    },
    inline_parent_dir_traversal_with_filename,
    r#"import("../utils/formatDate");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "mfWidgets.formatDate" */"../utils/formatDate");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: Some("/project/mf-widgets/src/components/Group/index.tsx".into()),
            source_dirs: None,
        })
    },
    inline_deeply_nested_relative_import,
    r#"import("../../GroupTitle");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "mfWidgets.GroupTitle" */"../../GroupTitle");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_dynamic_import_inside_async_function,
    // The visitor must recurse into function bodies and annotate nested imports.
    r#"async function load() { return import("lodash/fp"); }"#,
    r#"async function load() { return import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash.fp" */"lodash/fp"); }"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_dynamic_import_in_arrow_then_callback,
    // Arrow function returning an import – verifies deep traversal.
    r#"const load = () => import("react");"#,
    r#"const load = () => import(/* webpackMode: "lazy-once" *//* webpackChunkName: "react" */"react");"#
);

test_inline!(
    Default::default(),
    |tester| {
        visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        })
    },
    inline_three_dynamic_imports_independent_chunk_names,
    r#"import("lodash"); import("react"); import("@scope/pkg");"#,
    r#"import(/* webpackMode: "lazy-once" *//* webpackChunkName: "lodash" */"lodash"); import(/* webpackMode: "lazy-once" *//* webpackChunkName: "react" */"react"); import(/* webpackMode: "lazy-once" *//* webpackChunkName: "scopePkg" */"@scope/pkg");"#
);

/// `test_inline!` uses an empty comment map when printing; this asserts real codegen output.
#[test]
fn emit_webpack_magic_comments_inside_import_call() {
    Tester::run(|tester| {
        let tr = visit_mut_pass(TransformVisitor {
            comments: tester.comments.clone(),
            filename: None,
            source_dirs: None,
        });
        let program = tester.apply_transform(
            tr,
            "input.js",
            Default::default(),
            None,
            r#"import("lodash/fp");"#,
        )?;

        let comments = tester.comments.clone();
        let out = tester.print(&program, &comments);
        let import_start = out.find("import(").expect("import call");
        let rest = &out[import_start..];
        let magic = rest.find("webpackChunkName").expect("webpackChunkName comment");
        let str_lit = rest.find(r#""lodash/fp""#).expect("module string");
        assert!(
            magic < str_lit,
            "chunk name magic comment must precede the string literal inside import(): {out:?}"
        );
        Ok(())
    });
}
