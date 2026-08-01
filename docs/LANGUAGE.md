# The Lumen Language

Lumen is a small, statically-typed, expression-oriented language. This is its
complete reference.

## Program structure

A program is a sequence of top-level items: **functions**, **constants**, and
**structs**. Execution begins at `main`, which must take no parameters and
return `unit`:

```lumen
fn main() {
    print_int(42);
}
```

Functions may be declared in any order and may be (mutually) recursive; a call
can refer to a function declared later in the file.

## Types

| Type        | Description                       | Literals                |
|-------------|-----------------------------------|-------------------------|
| `i64`       | 64-bit signed integer             | `0`, `42`, `1000`       |
| `f64`       | 64-bit IEEE-754 float             | `3.14`, `0.5`           |
| `bool`      | boolean                           | `true`, `false`         |
| `str`       | immutable UTF-8 string            | `"hello"`, `"a\nb"`     |
| `unit`      | the empty "no value" type         | (implicit)              |
| `[T]`       | array of a primitive element type | `[1, 2, 3]`             |
| `(A, B, …)` | tuple of two or more types        | `(1, true)`             |
| `struct`    | named record of fields            | `Point { x: 1, y: 2 }`  |

There are **no implicit conversions**: `i64` and `f64` never mix in an operator,
and an operator requires both operands to have the same type. The `to_float`
and `to_int` builtins convert explicitly.

## Functions

```lumen
fn add(a: i64, b: i64) -> i64 {
    a + b
}
```

A parameter list is comma-separated `name: type` pairs. The return type follows
`->`; if omitted, the function returns `unit`. The body is a block whose value is
returned implicitly, or control may leave early via `return`.

A value-returning function must produce a value on every path: either a trailing
expression or a `return` on each branch. Falling off the end of a non-`unit`
function is a compile error (`E0306`).

## Constants

```lumen
const LIMIT: i64 = 100;
const PI: f64 = 3.14159;
```

A `const` is a named compile-time value of a primitive type. Each use is inlined
as its literal value; a constant initialiser must itself be constant (`E0313`).

## Structs

```lumen
struct Point { x: i64, y: i64 }

fn main() {
    let mut p = Point { x: 1, y: 2 };
    p.x = p.x + p.y;
    print_int(p.x);
}
```

A struct literal must give every declared field exactly once; missing, extra,
duplicate, or unknown fields are an error (`E0317`). Fields are read and written
with `.name`. Structs have reference semantics: assigning a struct value shares
its storage rather than copying it.

## Arrays

```lumen
let xs = [10, 20, 30];
let first = xs[0];
xs[1] = 99;
let n = len(xs);
```

An array literal `[e0, e1, …]` has a primitive element type. Elements are read
and written with `[index]`; an out-of-bounds index is a runtime error. `len`
returns the element count. Arrays have reference semantics.

An array can also be allocated at a length that is only known while the program
runs, which is what a sieve, a DP table, or a histogram needs:

```lumen
fn zeroes(n: i64) -> [i64] {
    let xs = array_new_int(n);   // n zeroes
    xs
}
```

Lumen has no generics, so there is one such builtin per element type:
`array_new_int`, `array_new_float`, `array_new_bool`, and `array_new_str`. Each
takes a length and returns an array of that many zero values (`0`, `0.0`,
`false`, or `""`). The result is an ordinary array, indistinguishable from a
literal of the same type.

A negative length is a runtime error, as is a length above 16,777,216. That
ceiling is deliberate: it bounds runaway allocation the way the step limit
bounds runaway looping, so an absurd length fails as a clean error rather than
by exhausting memory.

## Tuples

```lumen
let pair = (1, true);
let a = pair.0;
let b = pair.1;
```

A tuple groups two or more values of possibly different types. Elements are read
positionally with `.0`, `.1`, and so on. A parenthesised single expression is
just grouping, not a one-element tuple.

## Bindings

```lumen
let x = 1;          // immutable, type inferred as i64
let y: f64 = 2.0;   // explicit annotation
let mut z = 0;      // mutable
z = z + 1;          // assignment (only to `mut` bindings)
z += 5;             // compound assignment
```

A `let` may shadow an earlier binding of the same name. Its initialiser is
evaluated in the *outer* scope, so `let n = n;` refers to a previously-visible
`n`, not the one being declared. Assigning to a non-`mut` binding is an error
(`E0307`). The compound forms `+=`, `-=`, `*=`, `/=`, and `%=` desugar to a plain
assignment.

## Expressions

Blocks, `if`, and `match`, along with arithmetic, are all expressions.

```lumen
let max = if a > b { a } else { b };

let scaled = {
    let half = a / 2;
    half + 1            // the block's value
};
```

An `if` used for its value must have an `else`, and both branches must have the
same type. An `if` without `else` has type `unit`.

### `match`

```lumen
fn label(n: i64) -> str {
    match n {
        0 => "zero",
        1 => "one",
        _ => "many",
    }
}
```

A `match` compares a scalar scrutinee (`i64` or `bool`) against literal patterns
or the wildcard `_`. Every arm must yield the same type, and the arms must be
exhaustive: cover every value, or include a `_` arm (`E0318`). A `match` lowers
to a chain of `if`/`else`, so it adds nothing to the runtime.

### Operators

From lowest to highest precedence:

| Precedence | Operators            | Associativity |
|------------|----------------------|---------------|
| 1          | `\|\|`               | left          |
| 2          | `&&`                 | left          |
| 3          | `==` `!=`            | left          |
| 4          | `<` `<=` `>` `>=`    | left          |
| 5          | `+` `-`              | left          |
| 6          | `*` `/` `%`          | left          |
| 7 (prefix) | `-` `!`              |               |

`&&` and `||` short-circuit: the right operand is not evaluated when the left
already determines the result. Assignment (`=`) is a separate, right-associative
form whose value is `unit`.

Arithmetic operators apply to `i64` and `f64`; comparisons (`<`, and so on) apply
to the numeric types; `==`/`!=` apply to any single type; `&&`/`||`/`!` apply to
`bool`. The `+` operator also concatenates two `str` values. Integer arithmetic
**wraps** on overflow. Integer division or remainder by zero is a runtime error.

## Statements

- `let [mut] name [: type] = expr;`
- `expr;` to evaluate for effect
- `return [expr];`
- `while cond { ... }` where `cond` must be `bool`
- `for v in start..end { ... }` over the half-open integer range `[start, end)`
- `for v in array { ... }` over each element of an array
- `break;` and `continue;` inside a loop

## Built-in functions

Lumen has no I/O syntax; output and the standard library are provided through
builtins, each with a fixed signature.

### Output

| Builtin               | Signature  |
|-----------------------|------------|
| `print_int(i64)`      | `-> unit`  |
| `print_float(f64)`    | `-> unit`  |
| `print_bool(bool)`    | `-> unit`  |
| `print_str(str)`      | `-> unit`  |

Each prints its argument followed by a newline.

### Conversions

| Builtin               | Signature  |
|-----------------------|------------|
| `int_to_str(i64)`     | `-> str`   |
| `float_to_str(f64)`   | `-> str`   |
| `bool_to_str(bool)`   | `-> str`   |
| `to_float(i64)`       | `-> f64`   |
| `to_int(f64)`         | `-> i64`   |
| `parse_int(str)`      | `-> i64`   |
| `char_to_str(i64)`    | `-> str`   |

`to_int` truncates toward zero; `parse_int` yields `0` on a malformed string;
`char_to_str` turns an ASCII byte value into a one-character string.

### Integer math

| Builtin                | Signature  |
|------------------------|------------|
| `abs(i64)`             | `-> i64`   |
| `sign(i64)`            | `-> i64`   |
| `min(i64, i64)`        | `-> i64`   |
| `max(i64, i64)`        | `-> i64`   |
| `clamp(i64, i64, i64)` | `-> i64`   |
| `pow_int(i64, i64)`    | `-> i64`   |
| `gcd(i64, i64)`        | `-> i64`   |
| `lcm(i64, i64)`        | `-> i64`   |

### Float math

| Builtin                 | Signature  |
|-------------------------|------------|
| `sqrt(f64)`             | `-> f64`   |
| `abs_float(f64)`        | `-> f64`   |
| `pow_float(f64, f64)`   | `-> f64`   |
| `min_float(f64, f64)`   | `-> f64`   |
| `max_float(f64, f64)`   | `-> f64`   |
| `floor(f64)`            | `-> i64`   |
| `ceil(f64)`             | `-> i64`   |
| `round(f64)`            | `-> i64`   |

### Strings and arrays

| Builtin                     | Signature  |
|-----------------------------|------------|
| `len([T])`                  | `-> i64`   |
| `array_new_int(i64)`        | `-> [i64]` |
| `array_new_float(i64)`      | `-> [f64]` |
| `array_new_bool(i64)`       | `-> [bool]`|
| `array_new_str(i64)`        | `-> [str]` |
| `str_len(str)`              | `-> i64`   |
| `substring(str, i64, i64)`  | `-> str`   |
| `char_at(str, i64)`         | `-> i64`   |
| `str_repeat(str, i64)`      | `-> str`   |
| `to_upper(str)`             | `-> str`   |
| `to_lower(str)`             | `-> str`   |
| `trim(str)`                 | `-> str`   |
| `starts_with(str, str)`     | `-> bool`  |
| `ends_with(str, str)`       | `-> bool`  |
| `contains(str, str)`        | `-> bool`  |
| `index_of(str, str)`        | `-> i64`   |

String builtins operate on byte indices; `index_of` returns `-1` when the needle
is absent.

## Comments

```lumen
// line comment
/* block comment, /* nested */ supported */
```

## Grammar (EBNF)

```ebnf
program  = item* ;
item     = fn | const | struct ;
fn       = "fn" ident "(" params? ")" ("->" type)? block ;
const    = "const" ident ":" type "=" expr ";" ;
struct   = "struct" ident "{" fields? "}" ;
fields   = field ("," field)* ","? ;
field    = ident ":" type ;
params   = param ("," param)* ","? ;
param    = ident ":" type ;
type     = ident | "[" type "]" | "(" type ("," type)+ ")" ;
block    = "{" stmt* expr? "}" ;
stmt     = "let" "mut"? ident (":" type)? "=" expr ";"
         | "return" expr? ";"
         | "while" expr block
         | "for" ident "in" expr (".." expr)? block
         | "break" ";"
         | "continue" ";"
         | expr ";" ;
expr     = assign ;
assign   = or (("="|"+="|"-="|"*="|"/="|"%=") assign)? ;
or       = and ("||" and)* ;
and      = cmp ("&&" cmp)* ;
cmp      = add (("=="|"!="|"<"|"<="|">"|">=") add)* ;
add      = mul (("+"|"-") mul)* ;
mul      = unary (("*"|"/"|"%") unary)* ;
unary    = ("-"|"!") unary | postfix ;
postfix  = primary ("(" args? ")" | "[" expr "]" | "." ident | "." int)* ;
primary  = int | float | string | "true" | "false" | ident
         | "[" args? "]" | "(" expr ("," expr)* ")" | block | if | match
         | ident "{" inits? "}" ;
if       = "if" expr block ("else" (if | block))? ;
match    = "match" expr "{" (pattern "=>" expr ","?)* "}" ;
pattern  = int | "-" int | "true" | "false" | "_" ;
```
