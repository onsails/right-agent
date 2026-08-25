//! AST-level guard for the single-owner database invariant.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Expr, FnArg, Item, ItemUse, Type, UseTree};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).expect("read source directory") {
        let entry = entry.expect("read source entry");
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        let syn::Meta::List(list) = &attribute.meta else {
            return false;
        };
        list.path.is_ident("cfg") && list.tokens.to_string().replace(' ', "") == "test"
    })
}

#[derive(Default)]
struct Aliases {
    targets: HashMap<String, Vec<Vec<String>>>,
}

impl Aliases {
    fn insert(&mut self, alias: String, target: Vec<String>) {
        self.targets.entry(alias).or_default().push(target);
    }

    fn resolved_paths(&self, path: &syn::Path) -> Vec<Vec<String>> {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let Some(first) = segments.first() else {
            return Vec::new();
        };
        let mut resolved = vec![segments.clone()];
        if let Some(targets) = self.targets.get(first) {
            for target in targets {
                let mut path = target.clone();
                path.extend_from_slice(&segments[1..]);
                resolved.push(path);
            }
        }
        resolved
    }
}

#[derive(Default)]
struct AliasCollector {
    aliases: Aliases,
}

impl<'ast> Visit<'ast> for AliasCollector {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        collect_use_tree(&item.tree, &mut Vec::new(), &mut self.aliases);
    }
}

fn collect_use_tree(tree: &UseTree, prefix: &mut Vec<String>, aliases: &mut Aliases) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(&path.tree, prefix, aliases);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut target = prefix.clone();
            target.push(name.ident.to_string());
            aliases.insert(name.ident.to_string(), target);
        }
        UseTree::Rename(rename) => {
            let mut target = prefix.clone();
            target.push(rename.ident.to_string());
            aliases.insert(rename.rename.to_string(), target);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix, aliases);
            }
        }
        UseTree::Glob(_) => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    RightDb,
    Connection,
    ProviderStore,
    LegacyRepair,
}

fn classify_open_path(path: &[String]) -> Option<OpenKind> {
    let last = path.last()?.as_str();
    if last == "repair_legacy_wal" && path.iter().any(|segment| segment == "right_db") {
        return Some(OpenKind::LegacyRepair);
    }
    if (last.starts_with("open_connection") || last.starts_with("open_database_path"))
        && (path.len() == 1 || path.iter().any(|segment| segment == "right_db"))
    {
        return Some(OpenKind::RightDb);
    }
    if last == "open_db" && path.iter().any(|segment| segment == "right_db") {
        return Some(OpenKind::RightDb);
    }
    if last.starts_with("open")
        && path
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|owner| owner == "Connection")
    {
        return Some(OpenKind::Connection);
    }
    if last == "open"
        && path
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|owner| owner == "ProviderStore")
    {
        return Some(OpenKind::ProviderStore);
    }
    None
}

fn expr_open_kind(expr: &Expr, aliases: &Aliases) -> Option<OpenKind> {
    let Expr::Path(expr_path) = expr else {
        return None;
    };
    aliases
        .resolved_paths(&expr_path.path)
        .into_iter()
        .find_map(|path| classify_open_path(&path))
}

fn type_carries_guard(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "RuntimeExclusionGuard" | "OfflineAgentDb"
            )
        }),
        Type::Reference(reference) => type_carries_guard(&reference.elem),
        Type::Paren(paren) => type_carries_guard(&paren.elem),
        Type::Group(group) => type_carries_guard(&group.elem),
        _ => false,
    }
}

fn named_pattern(pattern: &syn::Pat) -> Option<String> {
    match pattern {
        syn::Pat::Ident(ident) if ident.ident != "_" => Some(ident.ident.to_string()),
        syn::Pat::Type(typed) => named_pattern(&typed.pat),
        _ => None,
    }
}

fn unwrap_try_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Try(expr) => unwrap_try_paren(&expr.expr),
        Expr::Paren(expr) => unwrap_try_paren(&expr.expr),
        Expr::Group(expr) => unwrap_try_paren(&expr.expr),
        _ => expr,
    }
}

fn expression_acquires_carrier(expr: &Expr) -> bool {
    match unwrap_try_paren(expr) {
        Expr::Await(awaited) => matches!(
            unwrap_try_paren(&awaited.base),
            Expr::Call(call)
                if matches!(&*call.func, Expr::Path(path) if path.path.segments.last().is_some_and(|segment| {
                    matches!(segment.ident.to_string().as_str(),
                        "require_runtime_quiesced" | "acquire_runtime_exclusion" | "resolve_agent_db")
                }))
        ),
        Expr::Match(expression) => {
            expression_acquires_carrier(&expression.expr)
                && expression.arms.iter().any(|arm| is_ok_pattern(&arm.pat))
                && expression
                    .arms
                    .iter()
                    .all(|arm| is_ok_pattern(&arm.pat) || expression_diverges(&arm.body))
        }
        _ => false,
    }
}

fn is_ok_pattern(pattern: &syn::Pat) -> bool {
    matches!(pattern, syn::Pat::TupleStruct(tuple) if tuple.path.segments.last().is_some_and(|segment| segment.ident == "Ok"))
}

fn expression_diverges(expression: &Expr) -> bool {
    match expression {
        Expr::Return(_) => true,
        Expr::Block(block) => block.block.stmts.last().is_some_and(|statement| {
            matches!(statement, syn::Stmt::Expr(expression, _) if expression_diverges(expression))
        }),
        _ => false,
    }
}

#[derive(Debug)]
struct OpenSite {
    function: String,
    kind: OpenKind,
    guard_held: bool,
}

struct OpenFinder<'a> {
    aliases: &'a Aliases,
    function: &'a str,
    active_guards: HashSet<String>,
    sites: Vec<OpenSite>,
}

impl OpenFinder<'_> {
    fn record(&mut self, kind: OpenKind) {
        self.sites.push(OpenSite {
            function: self.function.to_owned(),
            kind,
            guard_held: !self.active_guards.is_empty(),
        });
    }

    fn scan_isolated(&mut self, expression: &Expr) {
        let mut isolated = OpenFinder {
            aliases: self.aliases,
            function: self.function,
            active_guards: HashSet::new(),
            sites: Vec::new(),
        };
        isolated.visit_expr(expression);
        self.sites.extend(isolated.sites);
    }
}

impl<'ast> Visit<'ast> for OpenFinder<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if is_test_only(&item.attrs) {
            return;
        }
        let nested_name = format!("{}::{}", self.function, item.sig.ident);
        self.sites.extend(scan_function_block(
            &item.block,
            self.aliases,
            &nested_name,
            function_guard_parameters(&item.sig),
        ));
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Some(kind) = expr_open_kind(&call.func, self.aliases) {
            self.record(kind);
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let Some(init) = &local.init
            && let Some(kind) = expr_open_kind(&init.expr, self.aliases)
        {
            // A callable constructor alias can be invoked after the tracked
            // guard is dropped or moved. Reject the binding itself rather than
            // attempting interprocedural value-flow proof.
            self.sites.push(OpenSite {
                function: self.function.to_owned(),
                kind,
                guard_held: false,
            });
        }
        syn::visit::visit_local(self, local);
    }

    fn visit_expr_block(&mut self, expression: &'ast syn::ExprBlock) {
        self.sites.extend(scan_function_block(
            &expression.block,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_async(&mut self, expression: &'ast syn::ExprAsync) {
        self.sites.extend(scan_function_block(
            &expression.block,
            self.aliases,
            self.function,
            HashSet::new(),
        ));
    }

    fn visit_expr_closure(&mut self, expression: &'ast syn::ExprClosure) {
        self.scan_isolated(&expression.body);
    }

    fn visit_expr_if(&mut self, expression: &'ast syn::ExprIf) {
        self.visit_expr(&expression.cond);
        self.sites.extend(scan_function_block(
            &expression.then_branch,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
        if let Some((_, otherwise)) = &expression.else_branch {
            self.visit_expr(otherwise);
        }
    }

    fn visit_expr_match(&mut self, expression: &'ast syn::ExprMatch) {
        self.visit_expr(&expression.expr);
        for arm in &expression.arms {
            self.visit_expr(&arm.body);
        }
    }

    fn visit_expr_loop(&mut self, expression: &'ast syn::ExprLoop) {
        self.sites.extend(scan_function_block(
            &expression.body,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_while(&mut self, expression: &'ast syn::ExprWhile) {
        self.visit_expr(&expression.cond);
        self.sites.extend(scan_function_block(
            &expression.body,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_for_loop(&mut self, expression: &'ast syn::ExprForLoop) {
        self.visit_expr(&expression.expr);
        self.sites.extend(scan_function_block(
            &expression.body,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_try_block(&mut self, expression: &'ast syn::ExprTryBlock) {
        self.sites.extend(scan_function_block(
            &expression.block,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_unsafe(&mut self, expression: &'ast syn::ExprUnsafe) {
        self.sites.extend(scan_function_block(
            &expression.block,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }

    fn visit_expr_const(&mut self, expression: &'ast syn::ExprConst) {
        self.sites.extend(scan_function_block(
            &expression.block,
            self.aliases,
            self.function,
            self.active_guards.clone(),
        ));
    }
}

struct MovedGuardFinder<'a> {
    active: &'a HashSet<String>,
    moved: HashSet<String>,
    borrowed: bool,
}

impl<'ast> Visit<'ast> for MovedGuardFinder<'_> {
    fn visit_expr_reference(&mut self, reference: &'ast syn::ExprReference) {
        let borrowed = self.borrowed;
        self.borrowed = true;
        self.visit_expr(&reference.expr);
        self.borrowed = borrowed;
    }

    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if !self.borrowed
            && path.path.segments.len() == 1
            && let Some(segment) = path.path.segments.first()
        {
            let name = segment.ident.to_string();
            if self.active.contains(&name) {
                self.moved.insert(name);
            }
        }
    }
}

fn moved_guards_in_arguments<'ast>(
    arguments: impl IntoIterator<Item = &'ast Expr>,
    active: &HashSet<String>,
) -> HashSet<String> {
    let mut finder = MovedGuardFinder {
        active,
        moved: HashSet::new(),
        borrowed: false,
    };
    for argument in arguments {
        finder.visit_expr(argument);
    }
    finder.moved
}

struct GuardInvalidationFinder<'a> {
    active: &'a HashSet<String>,
    invalidated: HashSet<String>,
}

impl<'ast> Visit<'ast> for GuardInvalidationFinder<'_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        self.invalidated
            .extend(moved_guards_in_arguments(&call.args, self.active));
        self.visit_expr(&call.func);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.invalidated.extend(moved_guards_in_arguments(
            std::iter::once(&*call.receiver).chain(call.args.iter()),
            self.active,
        ));
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let Some(init) = &local.init {
            self.invalidated.extend(moved_guards_in_arguments(
                std::iter::once(&*init.expr),
                self.active,
            ));
        }
    }
}

fn scan_function_block(
    block: &syn::Block,
    aliases: &Aliases,
    function: &str,
    mut active_guards: HashSet<String>,
) -> Vec<OpenSite> {
    let mut sites = Vec::new();
    for statement in &block.stmts {
        let mut invalidations = GuardInvalidationFinder {
            active: &active_guards,
            invalidated: HashSet::new(),
        };
        invalidations.visit_stmt(statement);
        for name in invalidations.invalidated {
            active_guards.remove(&name);
        }

        let mut finder = OpenFinder {
            aliases,
            function,
            active_guards: active_guards.clone(),
            sites: Vec::new(),
        };
        finder.visit_stmt(statement);
        sites.extend(finder.sites);

        if let syn::Stmt::Local(local) = statement
            && let Some(init) = &local.init
            && expression_acquires_carrier(&init.expr)
            && let Some(name) = named_pattern(&local.pat)
        {
            active_guards.insert(name);
        }
    }
    sites
}

fn function_guard_parameters(signature: &syn::Signature) -> HashSet<String> {
    signature
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Receiver(_) => None,
            FnArg::Typed(argument) if type_carries_guard(&argument.ty) => {
                named_pattern(&argument.pat)
            }
            FnArg::Typed(_) => None,
        })
        .collect()
}

fn type_name(ty: &Type) -> String {
    let Type::Path(path) = ty else {
        return "<impl>".to_owned();
    };
    path.path
        .segments
        .last()
        .map_or_else(|| "<impl>".to_owned(), |segment| segment.ident.to_string())
}

fn scan_items(
    items: &[Item],
    modules: &mut Vec<String>,
    aliases: &Aliases,
    out: &mut Vec<OpenSite>,
) {
    for item in items {
        match item {
            Item::Fn(function) if !is_test_only(&function.attrs) => {
                let mut path = modules.clone();
                path.push(function.sig.ident.to_string());
                out.extend(scan_function_block(
                    &function.block,
                    aliases,
                    &path.join("::"),
                    function_guard_parameters(&function.sig),
                ));
            }
            Item::Impl(item_impl) if !is_test_only(&item_impl.attrs) => {
                let owner = type_name(&item_impl.self_ty);
                for member in &item_impl.items {
                    let syn::ImplItem::Fn(function) = member else {
                        continue;
                    };
                    if is_test_only(&function.attrs) {
                        continue;
                    }
                    let mut path = modules.clone();
                    path.push(owner.clone());
                    path.push(function.sig.ident.to_string());
                    out.extend(scan_function_block(
                        &function.block,
                        aliases,
                        &path.join("::"),
                        function_guard_parameters(&function.sig),
                    ));
                }
            }
            Item::Mod(module) if !is_test_only(&module.attrs) => {
                if let Some((_, contents)) = &module.content {
                    modules.push(module.ident.to_string());
                    scan_items(contents, modules, aliases, out);
                    modules.pop();
                }
            }
            _ => {}
        }
    }
}

fn scan_source(source: &str) -> Result<Vec<OpenSite>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut alias_collector = AliasCollector::default();
    alias_collector.visit_file(&file);
    let mut sites = Vec::new();
    scan_items(
        &file.items,
        &mut Vec::new(),
        &alias_collector.aliases,
        &mut sites,
    );
    Ok(sites)
}

#[derive(Clone, Copy)]
enum Capability {
    RuntimeOwner,
    GuardedOffline,
    ScopedSpecial,
}

#[derive(Clone, Copy)]
struct Allowance {
    file: &'static str,
    function: &'static str,
    capability: Capability,
}

const ALLOWANCES: &[Allowance] = &[
    Allowance {
        file: "crates/right/src/db_owner.rs",
        function: "AgentDbOwner::open_and_migrate",
        capability: Capability::RuntimeOwner,
    },
    Allowance {
        file: "crates/right/src/internal_api.rs",
        function: "open_provider_store",
        capability: Capability::RuntimeOwner,
    },
    Allowance {
        file: "crates/right/src/db_repair.rs",
        function: "run_db_repair",
        capability: Capability::ScopedSpecial,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "cmd_init",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "cmd_agent_init",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "persist_claude_setup_token",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "cmd_agent_restore",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "open_provider_store_for_restore",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "migrate_restored_agent_db",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "cmd_agent_backup",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/main.rs",
        function: "resolve_agent_db",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/migrate_sandbox.rs",
        function: "cmd_agent_migrate_sandbox",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right/src/restore.rs",
        function: "explicit_state_manifest",
        capability: Capability::ScopedSpecial,
    },
    Allowance {
        file: "crates/right-agent/src/doctor.rs",
        function: "check_mcp_tokens_impl",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/doctor.rs",
        function: "check_cron_targets",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/doctor.rs",
        function: "check_cron_runs",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/doctor.rs",
        function: "check_memory",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/rebootstrap.rs",
        function: "deactivate_active_sessions",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/rebootstrap.rs",
        function: "clear_bootstrap_answers",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/agent/destroy.rs",
        function: "run_backup",
        capability: Capability::GuardedOffline,
    },
    Allowance {
        file: "crates/right-agent/src/agent/destroy.rs",
        function: "destroy_agent",
        capability: Capability::GuardedOffline,
    },
];

fn policy_violations(relative: &str, source: &str, allowances: &[Allowance]) -> Vec<String> {
    let sites = match scan_source(source) {
        Ok(sites) => sites,
        Err(error) => return vec![format!("{relative}: cannot parse Rust source: {error}")],
    };
    sites
        .into_iter()
        .filter_map(|site| {
            let allowance = allowances.iter().find(|allowance| {
                allowance.file == relative && allowance.function == site.function
            });
            match allowance {
                None => Some(format!(
                    "{relative}: {} uses {:?} without an exact capability allowance",
                    site.function, site.kind
                )),
                Some(Allowance {
                    capability: Capability::GuardedOffline,
                    ..
                }) if !site.guard_held => Some(format!(
                    "{relative}: {} uses {:?} before acquiring or receiving RuntimeExclusionGuard",
                    site.function, site.kind
                )),
                Some(_) => None,
            }
        })
        .collect()
}

#[test]
fn scanner_rejects_alias_multiline_qualified_and_cross_function_guard_bypasses() {
    let cases = [
        (
            "alias",
            "use right_db::open_connection as connect; fn bypass() { connect(&dir, false); }",
        ),
        (
            "multiline",
            "fn bypass() { right_db::open_connection\n (&dir, false); }",
        ),
        (
            "qualified",
            "use right_db as db; fn bypass() { db::open_database_path(&path); }",
        ),
        (
            "connection alias",
            "use right_db::Connection as DbConnection; fn bypass() { DbConnection::open_local(path, true); }",
        ),
        (
            "provider alias",
            "use right_providers::ProviderStore as Store; fn bypass() { Store::open(home); }",
        ),
    ];
    for (name, source) in cases {
        assert!(
            !policy_violations("fixture.rs", source, &[]).is_empty(),
            "{name} bypass must be rejected"
        );
    }

    let unrelated_guard = r#"
        async fn guarded() {
            let _guard = require_runtime_quiesced(home).await?;
        }
        async fn offline_open() {
            right_db::open_connection(home, false).await?;
        }
    "#;
    let allowance = [Allowance {
        file: "fixture.rs",
        function: "offline_open",
        capability: Capability::GuardedOffline,
    }];
    assert_eq!(
        policy_violations("fixture.rs", unrelated_guard, &allowance).len(),
        1
    );
}

#[test]
fn scanner_rejects_discarded_unawaited_conditional_and_dropped_guards() {
    let cases = [
        (
            "discarded",
            r#"async fn offline_open() {
                let _ = require_runtime_quiesced(home).await?;
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "unawaited",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home);
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "conditional",
            r#"async fn offline_open() {
                if condition {
                    let guard = require_runtime_quiesced(home).await?;
                }
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "dropped",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                drop(guard);
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "moved",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                let transferred = guard;
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "qualified drop",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                std::mem::drop(guard);
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "consumed argument",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                consume(guard);
                right_db::open_connection(home, false).await?;
            }"#,
        ),
        (
            "closure body",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                let f = || right_db::open_connection(home, false);
            }"#,
        ),
        (
            "async argument",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                spawn(async { right_db::open_connection(home, false).await });
            }"#,
        ),
        (
            "opener call argument",
            r#"async fn offline_open() {
                consume(right_db::open_connection(home, false).await?);
            }"#,
        ),
        (
            "local function alias",
            r#"fn offline_open() {
                let connect = right_db::open_connection;
                connect(home, false);
            }"#,
        ),
        (
            "local provider alias",
            r#"fn offline_open() {
                let open = ProviderStore::open;
                open(home);
            }"#,
        ),
        (
            "opener local initializer",
            r#"async fn offline_open() {
                let connection = Some(right_db::open_connection(home, false).await?);
            }"#,
        ),
        (
            "nested function",
            r#"async fn offline_open() {
                let guard = require_runtime_quiesced(home).await?;
                fn nested() {
                    right_db::open_connection(home, false);
                }
            }"#,
        ),
    ];
    let allowance = [Allowance {
        file: "fixture.rs",
        function: "offline_open",
        capability: Capability::GuardedOffline,
    }];
    for (name, source) in cases {
        assert_eq!(
            policy_violations("fixture.rs", source, &allowance).len(),
            1,
            "{name} guard bypass must be rejected"
        );
    }
}

#[test]
fn scanner_accepts_guard_in_same_function_before_open_or_guard_parameter() {
    let acquired = r#"
        async fn offline_open() {
            let _guard = require_runtime_quiesced(home).await?;
            right_db::open_connection(home, false).await?;
        }
    "#;
    let received = r#"
        async fn offline_open(_guard: &RuntimeExclusionGuard) {
            right_db::open_connection(home, false).await?;
        }
    "#;
    let allowance = [Allowance {
        file: "fixture.rs",
        function: "offline_open",
        capability: Capability::GuardedOffline,
    }];
    assert!(policy_violations("fixture.rs", acquired, &allowance).is_empty());
    assert!(policy_violations("fixture.rs", received, &allowance).is_empty());
}

#[test]
fn scanner_ignores_comments_strings_and_cfg_test_but_not_cfg_any() {
    let source = r#"
        fn clean() {
            // right_db::open_connection(home, false);
            let fake = "ProviderStore::open(home)";
        }
        #[cfg(test)] mod tests { fn helper() { right_db::open_connection(home, false); } }
        #[cfg(any())] mod hidden { fn bypass() { right_db::open_connection(home, false); } }
    "#;
    let violations = policy_violations("fixture.rs", source, &[]);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("hidden::bypass"));
}

#[test]
fn bot_runtime_has_no_direct_database_access() {
    let root = workspace_root().join("crates/bot/src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut violations = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(workspace_root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative.ends_with("_tests.rs") || relative.ends_with("crates/bot/src/keepalive.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read Rust source");
        violations.extend(policy_violations(&relative, &source, &[]));
    }
    assert!(
        violations.is_empty(),
        "bot runtime direct DB access:\n{}",
        violations.join("\n")
    );
}

#[test]
fn right_backend_never_opens_or_exposes_a_connection() {
    let path = workspace_root().join("crates/right/src/right_backend.rs");
    let source = std::fs::read_to_string(&path).expect("read right_backend.rs");
    assert!(
        scan_source(&source)
            .expect("parse right_backend.rs")
            .is_empty(),
        "right_backend.rs must not construct a database connection"
    );
    assert!(
        !source.contains("get_conn("),
        "right_backend.rs must not expose a connection"
    );
    assert!(
        source.contains("with_db("),
        "owner-scoped helper must remain in use"
    );
}

#[test]
fn production_direct_openers_are_exactly_allowlisted_and_guarded() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for crate_path in [
        "right/src",
        "right-mcp/src",
        "right-memory/src",
        "right-agent/src",
    ] {
        let mut files = Vec::new();
        rust_files(&root.join("crates").join(crate_path), &mut files);
        for path in files {
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative.ends_with("_tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read Rust source");
            violations.extend(policy_violations(&relative, &source, ALLOWANCES));
        }
    }
    assert!(
        violations.is_empty(),
        "direct DB opener policy violations:\n{}",
        violations.join("\n")
    );
}
