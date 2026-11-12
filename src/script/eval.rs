//! Evaluates forms.
//!
//! The language is described in `docs/scripting-language.md`. In short: a
//! list is a call unless its first element is one of the keywords below, and
//! everything is an expression.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::builtins;
use crate::input::Input;
use super::reader::{Error, Form, FormKind, Pos};
use super::value::{Function, Value};

/// Words with a fixed meaning. They cannot name a variable.
const KEYWORDS: [&str; 8] = ["var", "set", "if", "do", "fn", "return", "and", "or"];

/// How many lists may be in the middle of being evaluated at once: each
/// nested expression and each function call counts. Evaluation recurses on
/// the native stack, so this is what turns runaway recursion in a script
/// into an error instead of a crash.
///
/// Sized against the stack `script::run` provides. See the measurements in
/// `docs/scripting-language.md`.
pub const MAX_DEPTH: usize = 2000;

/// What running a script produced.
#[derive(Debug)]
pub struct Outcome {
    /// The value of the last form, or whatever a top-level `return` gave.
    pub value: Value,
    /// Everything outside the script that the value depends on. While these
    /// are unchanged, running the script again would give the same value.
    pub inputs: Vec<Input>,
}

/// Evaluates the top-level forms of a script. Relative paths in the script
/// are resolved against `base`.
pub(super) fn run(forms: &[Form], base: &Path) -> Result<Outcome, Error> {
    let mut interp = Interp { depth: 0, base: base.to_path_buf(), inputs: Vec::new() };
    let scope = Rc::new(Scope::default());
    let value = match interp.block(forms, &scope) {
        Ok(value) | Err(Flow::Return(value)) => value,
        Err(Flow::Error(error)) => return Err(error),
    };
    Ok(Outcome { value, inputs: interp.inputs })
}

/// The variables of one block or function call, and the scope around it.
///
/// A function keeps the scope it was defined in alive, and that scope holds
/// the function when it is named, so such pairs are never freed. A script
/// runs once and exits, so nothing is done about it.
#[derive(Default)]
pub struct Scope {
    vars: RefCell<Vec<(String, Value)>>,
    parent: Option<Rc<Scope>>,
}

impl Scope {
    fn inside(parent: &Rc<Scope>) -> Rc<Scope> {
        Rc::new(Scope { vars: RefCell::default(), parent: Some(Rc::clone(parent)) })
    }

    fn get(&self, name: &str) -> Option<Value> {
        let vars = self.vars.borrow();
        match vars.iter().find(|(known, _)| known == name) {
            Some((_, value)) => Some(value.clone()),
            None => self.parent.as_ref()?.get(name),
        }
    }

    /// Adds a variable to this scope. Returns false if it already has one
    /// with that name.
    fn define(&self, name: &str, value: Value) -> bool {
        let mut vars = self.vars.borrow_mut();
        if vars.iter().any(|(known, _)| known == name) {
            return false;
        }
        vars.push((name.to_string(), value));
        true
    }

    /// Changes the nearest variable with that name. Returns false if there
    /// is none.
    fn assign(&self, name: &str, value: Value) -> bool {
        let mut vars = self.vars.borrow_mut();
        match vars.iter_mut().find(|(known, _)| known == name) {
            Some(slot) => {
                slot.1 = value;
                true
            }
            None => {
                drop(vars);
                self.parent.as_ref().is_some_and(|parent| parent.assign(name, value))
            }
        }
    }
}

/// Why evaluation of a form stopped early.
enum Flow {
    /// `return`, on its way out to the enclosing function or the script.
    Return(Value),
    Error(Error),
}

impl From<Error> for Flow {
    fn from(error: Error) -> Flow {
        Flow::Error(error)
    }
}

fn error(pos: Pos, message: impl Into<String>) -> Error {
    Error { pos, message: message.into() }
}

pub struct Interp {
    depth: usize,
    base: PathBuf,
    inputs: Vec<Input>,
}

impl Interp {
    /// Where a path written in the script points, relative to the current
    /// directory. An empty path is the base directory itself.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = if path.is_empty() { "." } else { path };
        if self.base.as_os_str().is_empty() {
            PathBuf::from(path)
        } else {
            self.base.join(path)
        }
    }

    /// Notes something the script's value now depends on.
    pub fn record(&mut self, input: Input) {
        if !self.inputs.contains(&input) {
            self.inputs.push(input);
        }
    }

    /// Calls a function value with arguments that are already evaluated.
    /// `pos` is where the call is, for error messages.
    pub fn call(&mut self, callee: &Value, args: &[Value], pos: Pos) -> Result<Value, Error> {
        match callee {
            Value::Native(builtin) => (builtin.call)(self, args, pos),
            Value::Fn(function) => {
                if args.len() != function.params.len() {
                    let name = function.name.as_deref().unwrap_or("this function");
                    return Err(error(
                        pos,
                        format!(
                            "`{name}` takes {} but was given {}",
                            count(function.params.len(), "argument"),
                            args.len()
                        ),
                    ));
                }
                let scope = Scope::inside(&function.scope);
                for (param, arg) in function.params.iter().zip(args) {
                    scope.define(param, arg.clone());
                }
                match self.block(&function.body, &scope) {
                    Ok(value) | Err(Flow::Return(value)) => Ok(value),
                    Err(Flow::Error(error)) => Err(error),
                }
            }
            other => Err(error(pos, format!("{} is not a function", other.type_name()))),
        }
    }

    /// Evaluates forms in order and gives the value of the last, or `()`.
    fn block(&mut self, forms: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let mut value = Value::nil();
        for form in forms {
            value = self.eval(form, scope)?;
        }
        Ok(value)
    }

    fn eval(&mut self, form: &Form, scope: &Rc<Scope>) -> Result<Value, Flow> {
        match &form.kind {
            FormKind::Bool(value) => Ok(Value::Bool(*value)),
            FormKind::Int(value) => Ok(Value::Int(*value)),
            FormKind::Str(text) => Ok(Value::str(text)),
            FormKind::Bytes(bytes) => Ok(Value::Bytes(Rc::from(bytes.as_slice()))),
            FormKind::Symbol(name) => match name.as_str() {
                "return" => Err(Flow::Return(Value::nil())),
                name if KEYWORDS.contains(&name) => Err(error(
                    form.pos,
                    format!("`{name}` is a keyword; it must come first in a list"),
                )
                .into()),
                name => scope
                    .get(name)
                    .or_else(|| builtins::lookup(name).map(Value::Native))
                    .ok_or_else(|| error(form.pos, format!("`{name}` is not defined")).into()),
            },
            FormKind::List(items) => {
                if self.depth == MAX_DEPTH {
                    return Err(error(
                        form.pos,
                        format!(
                            "evaluation is nested more than {MAX_DEPTH} deep; \
                             a function may be calling itself without end"
                        ),
                    )
                    .into());
                }
                self.depth += 1;
                let result = self.list(form.pos, items, scope);
                self.depth -= 1;
                result
            }
        }
    }

    fn list(&mut self, pos: Pos, items: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let Some((head, rest)) = items.split_first() else {
            return Ok(Value::nil());
        };
        if let FormKind::Symbol(name) = &head.kind {
            match name.as_str() {
                "var" => return self.var(pos, rest, scope),
                "set" => return self.set(pos, rest, scope),
                "if" => return self.if_(pos, rest, scope),
                "do" => return self.block(rest, &Scope::inside(scope)),
                "fn" => return self.function(pos, rest, scope),
                "return" => return self.return_(pos, rest, scope),
                "and" => return self.and(rest, scope),
                "or" => return self.or(rest, scope),
                _ => {}
            }
        }
        let callee = self.eval(head, scope)?;
        let mut args = Vec::with_capacity(rest.len());
        for form in rest {
            args.push(self.eval(form, scope)?);
        }
        Ok(self.call(&callee, &args, pos)?)
    }

    /// `(var name value)`
    fn var(&mut self, pos: Pos, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let [name, value] = args else {
            return Err(error(pos, "`var` takes a name and a value: (var name value)").into());
        };
        let name = new_name(name)?;
        let value = self.eval(value, scope)?;
        if !scope.define(name, value) {
            return Err(error(
                pos,
                format!("`{name}` is already defined here; use `set` to change it"),
            )
            .into());
        }
        Ok(Value::nil())
    }

    /// `(set name value)`
    fn set(&mut self, pos: Pos, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let [name, value] = args else {
            return Err(error(pos, "`set` takes a name and a value: (set name value)").into());
        };
        let name = new_name(name)?;
        let value = self.eval(value, scope)?;
        if !scope.assign(name, value) {
            let message = if builtins::lookup(name).is_some() {
                format!("`{name}` is built in and cannot be changed")
            } else {
                format!("`{name}` is not defined; use `var` to create it")
            };
            return Err(error(pos, message).into());
        }
        Ok(Value::nil())
    }

    /// `(if condition then)` or `(if condition then else)`
    fn if_(&mut self, pos: Pos, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let (condition, then, otherwise) = match args {
            [condition, then] => (condition, then, None),
            [condition, then, otherwise] => (condition, then, Some(otherwise)),
            _ => {
                return Err(error(
                    pos,
                    "`if` takes a condition, a result, and optionally another result",
                )
                .into());
            }
        };
        if self.eval(condition, scope)?.is_truthy() {
            self.eval(then, scope)
        } else {
            otherwise.map_or_else(|| Ok(Value::nil()), |form| self.eval(form, scope))
        }
    }

    /// `(fn name (params...) body...)` defines a function and binds it to
    /// `name`. `(fn (params...) body...)` evaluates to an anonymous one.
    fn function(&mut self, pos: Pos, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let usage = "`fn` takes an optional name, a list of parameters, and a body";
        let (name, rest) = match args.split_first() {
            Some((first, rest)) if matches!(first.kind, FormKind::Symbol(_)) => {
                (Some(new_name(first)?), rest)
            }
            _ => (None, args),
        };
        let Some((Form { kind: FormKind::List(params), .. }, body)) = rest.split_first() else {
            return Err(error(pos, usage).into());
        };
        let mut names: Vec<String> = Vec::with_capacity(params.len());
        for param in params {
            let param_name = new_name(param)?;
            if names.iter().any(|known| known == param_name) {
                return Err(error(
                    param.pos,
                    format!("parameter `{param_name}` is listed twice"),
                )
                .into());
            }
            names.push(param_name.to_string());
        }

        let function = Value::Fn(Rc::new(Function {
            name: name.map(str::to_string),
            params: names,
            body: body.to_vec(),
            scope: Rc::clone(scope),
        }));
        match name {
            None => Ok(function),
            Some(name) if scope.define(name, function) => Ok(Value::nil()),
            Some(name) => Err(error(pos, format!("`{name}` is already defined here")).into()),
        }
    }

    /// `(return)` or `(return value)`. A bare `return` is handled in `eval`.
    fn return_(&mut self, pos: Pos, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let value = match args {
            [] => Value::nil(),
            [value] => self.eval(value, scope)?,
            _ => return Err(error(pos, "`return` takes at most one value").into()),
        };
        Err(Flow::Return(value))
    }

    /// `(and a b ...)`: the first false value, otherwise the last value.
    fn and(&mut self, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let mut value = Value::Bool(true);
        for form in args {
            value = self.eval(form, scope)?;
            if !value.is_truthy() {
                break;
            }
        }
        Ok(value)
    }

    /// `(or a b ...)`: the first true value, otherwise the last value.
    fn or(&mut self, args: &[Form], scope: &Rc<Scope>) -> Result<Value, Flow> {
        let mut value = Value::Bool(false);
        for form in args {
            value = self.eval(form, scope)?;
            if value.is_truthy() {
                break;
            }
        }
        Ok(value)
    }
}

/// Reads a form as the name of something being defined or changed.
fn new_name(form: &Form) -> Result<&str, Error> {
    match &form.kind {
        FormKind::Symbol(name) if KEYWORDS.contains(&name.as_str()) => {
            Err(error(form.pos, format!("`{name}` is a keyword and cannot be used as a name")))
        }
        FormKind::Symbol(name) => Ok(name),
        _ => Err(error(form.pos, "expected a name")),
    }
}

/// "1 argument", "2 arguments".
pub fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::script;

    /// The value of a script, printed.
    fn value(source: &str) -> String {
        match script::run(source.as_bytes(), Path::new(""), |outcome| outcome.value.to_string()) {
            Ok(value) => value,
            Err(error) => panic!("{source}\nfailed: {error}"),
        }
    }

    fn failure(source: &str) -> String {
        match script::run(source.as_bytes(), Path::new(""), |outcome| outcome.value.to_string()) {
            Ok(value) => panic!("{source}\nshould fail but gave {value}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn literals_evaluate_to_themselves() {
        assert_eq!(value("42"), "42");
        assert_eq!(value("-7"), "-7");
        assert_eq!(value("true"), "true");
        assert_eq!(value("\"a \\\"b\\\"\\n\""), "\"a \\\"b\\\"\\n\"");
        assert_eq!(value("#3:abc"), "<3 bytes>");
        assert_eq!(value("()"), "()");
        assert_eq!(value(""), "()");
    }

    #[test]
    fn a_script_is_worth_its_last_form() {
        assert_eq!(value("1 2 3"), "3");
        assert_eq!(value("(var x 5) x"), "5");
        assert_eq!(value("(var x 5)"), "()");
    }

    #[test]
    fn arithmetic_and_comparison() {
        assert_eq!(value("(+ 1 2 3)"), "6");
        assert_eq!(value("(- 10 3 2)"), "5");
        assert_eq!(value("(- 4)"), "-4");
        assert_eq!(value("(* 2 3 4)"), "24");
        assert_eq!(value("(/ 7 2)"), "3");
        assert_eq!(value("(/ -7 2)"), "-3");
        assert_eq!(value("(% 7 3)"), "1");
        assert_eq!(value("(list (< 1 2) (> 1 2) (<= 2 2) (>= 1 2))"), "(true false true false)");
        assert_eq!(value("(list (< \"a\" \"b\") (>= \"b\" \"b\"))"), "(true true)");
        assert_eq!(value("(list (= 1 1) (= 1 \"1\") (= \"a\" \"a\") (= () ()) (= (list 1 2) (list 1 2)))"), "(true false true true true)");
        assert_eq!(value("(list (not true) (not ()) (not 0) (not \"\"))"), "(false true false false)");
    }

    #[test]
    fn only_false_and_the_empty_value_are_false() {
        assert_eq!(value("(if false 1 2)"), "2");
        assert_eq!(value("(if () 1 2)"), "2");
        assert_eq!(value("(if 0 1 2)"), "1");
        assert_eq!(value("(if \"\" 1 2)"), "1");
        assert_eq!(value("(if (list 1) 1 2)"), "1");
        assert_eq!(value("(if false 1)"), "()");
    }

    #[test]
    fn if_evaluates_only_the_branch_it_takes() {
        assert_eq!(value("(if true 1 (undefined))"), "1");
        assert_eq!(value("(if false (undefined) 2)"), "2");
    }

    #[test]
    fn and_or_stop_early_and_give_values() {
        assert_eq!(value("(and)"), "true");
        assert_eq!(value("(or)"), "false");
        assert_eq!(value("(and 1 2 3)"), "3");
        assert_eq!(value("(and 1 () (undefined))"), "()");
        assert_eq!(value("(or false () 7 (undefined))"), "7");
        assert_eq!(value("(or false ())"), "()");
    }

    #[test]
    fn variables_and_scopes() {
        assert_eq!(value("(var x 1) (set x (+ x 1)) x"), "2");
        // `do` has its own scope: it can change outer variables, and its own
        // do not leak out.
        assert_eq!(value("(var x 1) (do (set x 5) (var y 2)) x"), "5");
        assert_eq!(value("(var x 1) (do (var x 2) x)"), "2");
        assert_eq!(value("(var x 1) (do (var x 2)) x"), "1");
        assert_eq!(value("(do 1 2 3)"), "3");
        assert_eq!(value("(do)"), "()");
        // A script may give a built-in's name to its own variable.
        assert_eq!(value("(var list 3) list"), "3");
    }

    #[test]
    fn functions() {
        assert_eq!(value("(fn double (n) (* n 2)) (double 21)"), "42");
        assert_eq!(value("(fn two () 1 2) (two)"), "2");
        assert_eq!(value("(fn nothing ()) (nothing)"), "()");
        assert_eq!(value("((fn (a b) (+ a b)) 1 2)"), "3");
        assert_eq!(value("(var add (fn (a b) (+ a b))) (add 1 2)"), "3");
        assert_eq!(value("(fn f () 1) f"), "<fn f>");
        assert_eq!(value("(fn (x) x)"), "<fn>");
        assert_eq!(value("+"), "<fn +>");
        // Defining a named function is a statement.
        assert_eq!(value("(fn f () 1)"), "()");
    }

    #[test]
    fn functions_can_be_passed_and_returned() {
        assert_eq!(value("(fn twice (f x) (f (f x))) (twice (fn (n) (* n 3)) 2)"), "18");
        assert_eq!(value("(fn adder (n) (fn (x) (+ x n))) ((adder 10) 5)"), "15");
        assert_eq!(value("(fn apply (f a b) (f a b)) (apply + 2 3)"), "5");
    }

    #[test]
    fn closures_share_the_variables_they_captured() {
        let source = "
            (fn counter ()
              (var n 0)
              (fn () (set n (+ n 1)) n))
            (var a (counter))
            (var b (counter))
            (a) (a)
            (list (a) (b))";
        assert_eq!(value(source), "(3 1)");
    }

    #[test]
    fn recursion() {
        assert_eq!(value("(fn fact (n) (if (<= n 1) 1 (* n (fact (- n 1))))) (fact 20)"), "2432902008176640000");
        assert_eq!(value("(fn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 15)"), "610");
        let mutual = "
            (fn even? (n) (if (= n 0) true (odd? (- n 1))))
            (fn odd? (n) (if (= n 0) false (even? (- n 1))))
            (even? 10)";
        assert_eq!(value(mutual), "true");
    }

    #[test]
    fn return_leaves_the_function_or_the_script() {
        assert_eq!(value("(fn f (n) (if (< n 0) (return \"negative\")) \"fine\") (list (f -1) (f 1))"), "(\"negative\" \"fine\")");
        assert_eq!(value("(fn f () (if true return) 1) (f)"), "()");
        // Only the innermost function returns.
        assert_eq!(value("(fn inner () (return 1) 2) (fn outer () (inner) 3) (outer)"), "3");
        // At the top level it ends the script.
        assert_eq!(value("1 (if true return) (undefined)"), "()");
        assert_eq!(value("(return 5) (undefined)"), "5");
        assert_eq!(value("(do (return 5)) 6"), "5");
        assert_eq!(value("(+ 1 (return 9))"), "9");
    }

    #[test]
    fn reports_errors_with_positions() {
        let cases = [
            ("nope", "1:1: `nope` is not defined"),
            ("(var x 1)\n  (nope x)", "2:4: `nope` is not defined"),
            ("(1 2)", "1:1: an integer is not a function"),
            ("(())", "1:1: the empty value is not a function"),
            ("(\"f\" 1)", "1:1: a string is not a function"),
            ("(fn f (a b) a) (f 1)", "1:16: `f` takes 2 arguments but was given 1"),
            ("((fn (a) a))", "1:1: `this function` takes 1 argument but was given 0"),
            ("(var x)", "1:1: `var` takes a name and a value: (var name value)"),
            ("(var 1 2)", "1:6: expected a name"),
            ("(var if 2)", "1:6: `if` is a keyword and cannot be used as a name"),
            ("(var x 1) (var x 2)", "1:11: `x` is already defined here; use `set` to change it"),
            ("(set y 1)", "1:1: `y` is not defined; use `var` to create it"),
            ("(set list 1)", "1:1: `list` is built in and cannot be changed"),
            ("(if true)", "1:1: `if` takes a condition, a result, and optionally another result"),
            ("(fn f)", "1:1: `fn` takes an optional name, a list of parameters, and a body"),
            ("(fn f (a a) a)", "1:10: parameter `a` is listed twice"),
            ("(fn f (1) a)", "1:8: expected a name"),
            ("(fn f () 1) (fn f () 2)", "1:13: `f` is already defined here"),
            ("(return 1 2)", "1:1: `return` takes at most one value"),
            ("(+ 1 if)", "1:6: `if` is a keyword; it must come first in a list"),
            ("(+ 1 \"a\")", "1:1: `+` needs an integer, but was given a string"),
            ("(/ 1 0)", "1:1: division by zero"),
            ("(% 1 0)", "1:1: division by zero"),
            ("(+ 9223372036854775807 1)", "1:1: `+` overflowed"),
            ("(- -9223372036854775808)", "1:1: `-` overflowed"),
            ("(/ 1)", "1:1: `/` takes 2 arguments but was given 1"),
            ("(+)", "1:1: `+` takes at least 1 argument but was given 0"),
            ("(< 1 \"a\")", "1:1: `<` cannot compare an integer with a string"),
            ("(not)", "1:1: `not` takes 1 argument but was given 0"),
            ("(1", "1:1: `(` is never closed"),
        ];
        for (source, expected) in cases {
            assert_eq!(failure(source), expected, "{source}");
        }
    }

    #[test]
    fn an_error_inside_a_function_points_at_its_source() {
        let source = "(fn f (x)\n  (+ x \"a\"))\n(f 1)";
        assert_eq!(failure(source), "2:3: `+` needs an integer, but was given a string");
    }

    #[test]
    fn runaway_recursion_is_an_error() {
        let message = failure("(fn forever (n) (forever (+ n 1))) (forever 0)");
        assert!(message.contains("nested more than 2000 deep"), "{message}");
        assert_eq!(value("(fn down (n) (if (= n 0) 0 (down (- n 1)))) (down 60)"), "0");
    }

    #[test]
    fn the_depth_limit_fits_the_smallest_stack() {
        // Scripts run on the stack that `script::run` gives them. If the
        // evaluator's frames grow enough that the limit no longer fits it,
        // this overflows the stack and takes the test run down with it,
        // which is the signal to lower MAX_DEPTH or raise the stack size.
        for source in [
            "(fn forever (n) (forever (+ n 1))) (forever 0)",
            "(fn forever (n) (if true (do (+ 1 (forever n))))) (forever 0)",
            // Recursing through a built-in puts its frames on the stack too.
            "(fn forever (n) (map (list n) forever)) (forever 0)",
            "(fn forever (n) (filter (list n) forever)) (forever 0)",
        ] {
            assert!(failure(source).contains("nested more than"), "{source}");
        }
    }

    #[test]
    fn recursion_can_go_deep_enough_to_walk_a_long_list() {
        let source = "
            (fn count (items) (if (is-empty items) 0 (+ 1 (count (rest items)))))
            (fn upto (n) (if (= n 0) () (append (upto (- n 1)) n)))
            (count (upto 500))";
        assert_eq!(value(source), "500");
    }
}
