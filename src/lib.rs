//! Rust implementation of JMESPath, a query language for JSON.
//!
//! # Compiling JMESPath expressions
//!
//! Use the `jmespath::Expression` struct to compile and execute JMESPath
//! expressions. The `Expression` struct can be used multiple times on
//! different values without having to recompile the expression.
//!
//! ```
//! use jmespath;
//!
//! let expr = jmespath::Expression::new("foo.bar | baz").unwrap();
//!
//! // Parse some JSON data into a JMESPath variable
//! let json_str = "{\"foo\":{\"bar\":{\"baz\":true}}}";
//! let data = jmespath::Variable::from_json(json_str).unwrap();
//!
//! // Search the data with the compiled expression
//! let result = expr.search(data).unwrap();
//! assert_eq!(true, result.as_boolean().unwrap());
//! ```
//!
//! You can get the original expression as a string and the parsed expression
//! AST from the `Expression` struct:
//!
//! ```
//! use jmespath;
//! use jmespath::ast::Ast;
//!
//! let expr = jmespath::Expression::new("foo").unwrap();
//! assert_eq!("foo", expr.as_str());
//! assert_eq!(&Ast::Field {name: "foo".to_string(), offset: 0}, expr.as_ast());
//! ```
//!
//! ## JMESPath variables
//!
//! In order to evaluate expressions against a known data type, the
//! `jmespath::Variable` enum is used as both the input and output type.
//! More specifically, `Rc<Variable>` (or `jmespath::RcVar`) is used to allow
//! shared, reference counted data to be used by the JMESPath interpreter at
//! runtime.
//!
//! Because `jmespath::Variable` implements `serde::ser::Serialize`, many
//! existing types can be searched without needing an explicit coercion,
//! and any type that needs coercion can be implemented using serde's macros
//! or code generation capabilities. Any value that implements the
//! `serde::ser::Serialize` trait can be searched without needing explicit
//! coercions. This includes a number of common types, including serde's
//! `serde_json::Value` enum.
//!
//! The return value of searching data with JMESPath is also an `RcVar` (an
//! `Rc<Variable>`). `Variable` has a number of helper methods that make
//! it a data type that can be used directly, or you can convert `Variable`
//! to any serde value implementing `serde::de::Deserialize`.
//!
//! # Custom Functions
//!
//! You can register custom functions with a JMESPath expression using the
//! ExpressionBuilder and a CustomFnRegistry.
//!
//! ```
//! use jmespath::{ExpressionBuilder, Context, RcVar, Variable};
//! use jmespath::functions::{CustomFunction, Signature, ArgumentType, CustomFnRegistry};
//!
//! let mut functions = CustomFnRegistry::new();
//!
//! // Create a function that returns string values as-is.
//! functions.register_function("str_identity", Box::new(CustomFunction::new(
//!     Signature::new(vec![ArgumentType::String], None, ArgumentType::Number),
//!     Box::new(|args: &[RcVar], _: &mut Context| Ok(args[0].clone()))
//! )));
//!
//! let expr = ExpressionBuilder::new("str_identity('foo')")
//!     .with_fn_registry(&functions)
//!     .build()
//!     .unwrap();
//!
//! assert_eq!("foo", expr.search(()).unwrap().as_string().unwrap());
//! ```

#![feature(specialization)]

#![cfg_attr(feature="clippy", feature(plugin))]
#![cfg_attr(feature="clippy", plugin(clippy))]

#[macro_use] extern crate lazy_static;

extern crate serde;
extern crate serde_json;

pub use errors::{Error, ErrorReason, RuntimeError};
pub use parser::{parse, ParseResult};
pub use lexer::tokenize;
pub use variable::Variable;

use std::fmt;
use std::rc::Rc;

use self::serde::Serialize;

use ast::Ast;
use functions::{FnRegistry, BuiltinFnRegistry};
use variable::Serializer;
use interpreter::{interpret, SearchResult};

pub mod ast;
pub mod functions;
pub mod interpreter;

mod parser;
mod lexer;
mod errors;
mod variable;

lazy_static! {
    static ref DEFAULT_FN_REGISTRY: BuiltinFnRegistry = BuiltinFnRegistry::new();
}

/// Ref counted JMESPath variable.
pub type RcVar = Rc<Variable>;


/// A compiled JMESPath expression.
pub struct Expression<'a> {
    ast: Ast,
    expression: String,
    fn_registry: &'a FnRegistry,
}

impl<'a> Expression<'a> {
    /// Creates a new JMESPath expression from an expression string.
    #[inline]
    pub fn new(expression: &str) -> Result<Expression<'a>, Error> {
        Ok(Expression {
            expression: expression.to_owned(),
            ast: try!(parse(expression)),
            fn_registry: &*DEFAULT_FN_REGISTRY,
        })
    }

    /// Returns the result of searching Serde data with the compiled expression.
    pub fn search<T: Serialize>(&self, data: T) -> SearchResult {
        let mut ser = Serializer::new();
        data.serialize(&mut ser).ok().unwrap();
        self.search_variable(&Rc::new(ser.unwrap()))
    }

    /// Returns the result of searching a JMESPath variable with the compiled expression.
    ///
    /// NOTE: This specific method could eventually be removed once specialization
    ///     lands in Rust. See https://github.com/rust-lang/rfcs/pull/1210
    pub fn search_variable(&self, data: &RcVar) -> SearchResult {
        let mut ctx = Context::new(&self.expression, &*self.fn_registry);
        interpret(data, &self.ast, &mut ctx)
    }

    /// Returns the JMESPath expression from which the Expression was compiled.
    pub fn as_str(&self) -> &str {
        &self.expression
    }

    /// Returns the AST of the parsed JMESPath expression.
    pub fn as_ast(&self) -> &Ast {
        &self.ast
    }
}

impl<'a> fmt::Display for Expression<'a> {
    /// Shows the jmespath expression as a string.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl<'a> fmt::Debug for Expression<'a> {
    /// Shows the jmespath expression as a string.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Equality comparison is based on the original string.
impl<'a> PartialEq for Expression<'a> {
    fn eq(&self, other: &Expression) -> bool {
        self.as_str() == other.as_str()
    }
}


/// ExpressionBuilder is used to build more complex expressions.
///
/// ExpressionBuilder also allows the injection of a custom AST. This
/// could be useful for statically parsing and compiling JMESPath
/// expression. Furthermore, ExpressionBuilder allows you to easily inject
/// custom JMESPath functions.
pub struct ExpressionBuilder<'a, 'b> {
    expression: &'a str,
    ast: Option<Ast>,
    fn_registry: Option<&'b FnRegistry>,
}

impl<'a, 'b> ExpressionBuilder<'a, 'b> {
    /// Creates a new ExpressionBuilder using the given JMESPath expression.
    pub fn new(expression: &'a str) -> ExpressionBuilder<'a, 'b> {
        ExpressionBuilder {
            expression: expression,
            ast: None,
            fn_registry: None,
        }
    }

    /// Uses a custom AST when building the Expression.
    pub fn with_ast(mut self, ast: Ast) -> ExpressionBuilder<'a, 'b> {
        self.ast = Some(ast);
        self
    }

    /// Uses a custom function registry when building the Expression.
    pub fn with_fn_registry(mut self, fn_registry: &'b FnRegistry)
        -> ExpressionBuilder<'a, 'b>
    {
        self.fn_registry = Some(fn_registry);
        self
    }

    /// Finalize and creates the Expression.
    pub fn build(self) -> Result<Expression<'b>, Error> {
        Ok(Expression {
            ast: match self.ast {
                Some(a) => a,
                None => try!(parse(self.expression)),
            },
            expression: self.expression.to_owned(),
            fn_registry: self.fn_registry.unwrap_or(&*DEFAULT_FN_REGISTRY)
        })
    }
}


/// Context object used for error reporting.
pub struct Context<'a> {
    /// Expression that is being interpreted.
    pub expression: &'a str,
    /// Function dispatcher
    pub fn_registry: &'a FnRegistry,
    /// Offset being evaluated
    pub offset: usize,
}

impl<'a> Context<'a> {
    /// Create a new context struct.
    #[inline]
    pub fn new(expression: &'a str, fn_registry: &'a FnRegistry) -> Context<'a> {
        Context {
            expression: expression,
            fn_registry: fn_registry,
            offset: 0,
        }
    }
}


#[cfg(test)]
mod test {
    use std::rc::Rc;

    use super::*;
    use super::ast::Ast;

    #[test]
    fn formats_expression_as_string_or_debug() {
        let expr = Expression::new("foo | baz").unwrap();
        assert_eq!("foo | baz/foo | baz", format!("{}/{:?}", expr, expr));
    }

    #[test]
    fn implements_partial_eq() {
        let a = Expression::new("@").unwrap();
        let b = Expression::new("@").unwrap();
        assert!(a == b);
    }

    #[test]
    fn can_evaluate_jmespath_expression() {
        let expr = Expression::new("foo.bar").unwrap();
        let var = Variable::from_json("{\"foo\":{\"bar\":true}}").unwrap();
        assert_eq!(Rc::new(Variable::Bool(true)), expr.search(var).unwrap());
    }

    #[test]
    fn can_get_expression_ast() {
        let expr = Expression::new("foo").unwrap();
        assert_eq!(&Ast::Field {offset: 0, name: "foo".to_string()}, expr.as_ast());
    }

    #[test]
    fn builder_can_create_with_custom_ast() {
        // Notice that we are parsing foo.bar, but using "@" as the AST.
        let expr = ExpressionBuilder::new("foo.bar")
            .with_ast(Ast::Identity { offset: 0 })
            .build()
            .unwrap();
        assert_eq!(Rc::new(Variable::Number(99.0)), expr.search(99).unwrap());
    }

    #[test]
    fn can_use_custom_fn_registry() {
        use interpreter::SearchResult;
        use functions::{Signature, Function, CustomFnRegistry, ArgumentType};

        struct CustomFunction {
            fn_signature: Signature,
        }

        impl CustomFunction {
            fn new() -> CustomFunction {
                CustomFunction {
                    fn_signature: Signature::new(vec![], None, ArgumentType::Bool),
                }
            }
        }

        impl Function for CustomFunction {
            fn signature(&self) -> &Signature {
                &self.fn_signature
            }

            fn evaluate(&self, _args: &[RcVar], _ctx: &mut Context) -> SearchResult {
                Ok(Rc::new(Variable::Bool(true)))
            }
        }

        let mut custom_functions = CustomFnRegistry::new();
        custom_functions.register_function("constantly_true", Box::new(CustomFunction::new()));
        let expr = ExpressionBuilder::new("constantly_true()")
            .with_fn_registry(&custom_functions)
            .build()
            .unwrap();
        assert_eq!(Rc::new(Variable::Bool(true)), expr.search(()).unwrap());
    }
}
