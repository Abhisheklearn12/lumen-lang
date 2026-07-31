

# Lumen

Un pequeño lenguaje de programación tipado estáticamente y su compilador, escrito en
Rust idiomático. Lumen procesa un programa a través de una tubería de compilación completa
y explícita (lexografía, análisis sintáctico, resolución de nombres, verificación de tipos,
una representación intermedia tipada, optimización y generación de código) y ejecuta el
resultado en una máquina virtual de bytecode basada en pilas.

Está diseñado como un estudio de ingeniería de compiladores profesional: correcto,
observable, exhaustivamente probado y lo suficientemente pequeño para leerse de principio a fin.

```lumen
fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    for i in 0..11 {
        print_int(fib(i));
    }
}
```

```console
$ lumenc run examples/fib.lm
0
1
1
2
3
5
8
13
21
34
55
```

## Arquitectura

El compilador es una secuencia explícita de fases, cada una con una API pública reducida,
sus propios diagnósticos y su propio tipo de datos. Ninguna fase muta la salida de otra.

![Lumen compiler pipeline: source → lexer → parser → AST → (name resolution, type checking) → HIR → optimizer → bytecode → VM → output](docs/assets/pipeline.png)

| Fase             | Módulo             | Salida                          |
|------------------|--------------------|---------------------------------|
| Analizador léxico| `lexer`            | `Vec<Token>`                    |
| Analizador sint. | `parser`           | `Ast` (Basado en Pratt)         |
| Resolución de nombres| `sema::resolve`| Tablas laterales `Resolution`   |
| Verificación de tipos| `sema::typeck` | Tablas laterales `Typeck`       |
| Descenso         | `hir`              | `Hir` tipado y sin azúcar sintáctica|
| Optimizador      | `opt`              | `Hir` en línea y plegado        |
| Gen. de código   | `backend::codegen` | Bytecode `Program`              |
| VM               | `backend::vm`      | Salida / valor del programa     |

Más allá de la ruta principal, la misma HIR tipada alimenta dos backends adicionales usados para
análisis y validación: una IR de nivel medio basada en CFG (`mir`) con su propio optimizador de flujo
de datos e intérprete, y un transpilador a C (`backend::c`) para el subconjunto escalar. Un verificador
de bytecode (`backend::verify`) revisa cualquier programa (incluyendo uno cargado desde un archivo
objeto) antes de que se ejecute.

La justificación completa del diseño está en [`docs/DESIGN.md`](docs/DESIGN.md); la referencia del
lenguaje está en [`docs/LANGUAGE.md`](docs/LANGUAGE.md).

## Capacidades actuales

### Lenguaje y sintaxis

- Analizador léxico escrito a mano de un solo paso, sin retroceso, lineal en longitud de la fuente
- 16 palabras reservadas; los nombres de tipos primitivos son identificadores contextuales en lugar de palabras reservadas
- Comentarios de línea y comentarios de bloque anidados
- Literales de enteros de 64 bits y flotantes de 64 bits
- Cadenas literales con escapes `\n`, `\t`, `\r`, `\0`, `\\` y `\"`
- Analizador sintáctico de expresiones Pratt (ascenso de precedencia)
- Funciones, constantes a nivel superior y declaraciones de estructuras
- Vinculaciones `let` con anotación de tipo opcional y `mut` opcional
- Asignación y asignación compuesta (`+=`, `-=`, `*=`, `/=`, `%=`)
- Expresiones `if`/`else`
- Bucles `while`
- Bucles `for` contados sobre un rango semiabierto
- Bucles `for`-each sobre arreglos
- `match` sobre literales enteros y booleanos con un brazo comodín
- `break` y `continue`
- Bloques como expresiones, con una expresión de cola opcional que suministra el valor
- Negación aritmética unaria y negación lógica
- 13 operadores binarios, incluyendo `&&` y `||` con cortocircuito
- Arreglos literales y lectura/escritura por índice
- Estructuras literales y acceso a campos
- Tuplas literales y acceso posicional
- Llamadas a funciones recursivas

### Sistema de tipos

- Verificación estática de tipos completa antes de que se ejecute cualquier código
- `i64`, `f64`, `bool`, `str`, `unit`
- Arreglos de `i64`, `f64`, `bool` o `str`
- Estructuras definidas por el usuario con campos nombrados y tipados
- Tipos de tupla estructural, internados por su lista de elementos
- Inferencia de tipos para vinculaciones `let`
- Sin conversiones implícitas de ningún tipo
- Análisis de divergencia, por lo que una función cuyo cuerpo siempre retorna no necesita expresión de cola
- Evaluación de constantes en tiempo de compilación para elementos `const`, en línea en cada uso
- Verificación de exhaustividad de `match` (un objeto de examen `bool` es exhaustivo con `true` y `false`; cualquier otro necesita un comodín)
- Las aridades distintas de tupla son tipos distintos
- Referencias hacia adelante a estructuras y funciones
- Un tipo de error dedicado que absorbe errores posteriores, para que una sola expresión inválida no se propague en cascada en cada verificación posterior

### Diagnósticos

- 30 códigos de error estables, de `E0001` a `E0318`, agrupados por fase en bloques de cien
- Los códigos son solo de adición; el significado de un código publicado queda congelado
- Etiquetas de múltiples segmentos: una etiqueta principal más cualquier número de etiquetas secundarias
- Notas y texto de ayuda en cualquier diagnóstico
- Renderizados a través de `miette` con un tema fijo y determinista
- Cada fase del front-end es tolerante a errores y reporta múltiples problemas en una sola ejecución
- Sugerencias "¿quisiste decir?" basadas en Levenshtein con un umbral escalado por longitud
- `lumenc explain <CÓDIGO>` imprime una explicación detallada para los 30 códigos
- Segmentos de desplazamiento en bytes (8 bytes, `Copy`) adjuntos a cada nodo AST y HIR
- Resolución de desplazamiento a `línea:columna` en `O(log n)` a través de un índice de líneas precomputado
- Columnas contadas en valores escalares Unicode en lugar de bytes

### Flujo de compilación

- Ocho fases con temporización independiente: lex, parse, resolve, typeck, lower, optimize, codegen, peephole
- Una única `Session` conecta las fases; cada fase permanece testeable de forma independiente
- Ninguna fase muta la salida de otra fase
- Los resultados de resolución y verificación de tipos residen en tablas laterales indexadas por `NodeId`, dejando el AST inmutable
- El front-end siempre se ejecuta hasta completarse para diagnósticos completos; el descenso se ejecuta solo cuando está libre de errores
- HIR tipada con eliminación de azúcar: `match` se convierte en una cadena `if`/`else`, la asignación compuesta se convierte en asignación simple, `for`-each se convierte en un bucle indexado sobre ranuras ocultas, las tuplas se convierten en estructuras, las constantes se enlazan en línea
- Asignación densa de `LocalId` por función, parámetros primero
- La compilación puede detenerse después de cualquier etapa para inspección

### Optimización

Sobre HIR, se ejecuta hasta un punto fijo de hasta ocho iteraciones:

- Enlace en línea de funciones de expresión pequeñas, puras y no recursivas
- Plegado de constantes
- Simplificación algebraica
- Eliminación de código muerto: código inalcanzable después de `return`, `while false`, y vinculaciones `let` puras sin usar
- Un predicado de pureza compartido único decide qué es seguro eliminar
- Totalmente determinista, por lo que la salida optimizada es reproducible

Sobre el bytecode generado:

- Hilado de saltos a través de cadenas de saltos incondicionales
- Eliminación de push/pop para valores que se computan y descartan inmediatamente
- Remapeo exacto de destinos de salto; una instrucción que es en sí misma un destino de salto nunca se elimina

### Bytecode y máquina virtual

- Enum de instrucciones tipado en lugar de bytes empacados
- Opcodes aritméticos y de orden monomórficos elegidos en la generación de código (`AddInt` versus `AddFloat`), por lo que la VM nunca inspecciona los tipos de operandos para estos
- Un único opcode de igualdad estructural que cubre todos los tipos, la única instrucción que realiza despacho sobre los operandos
- Destinos de salto absolutos resueltos por retro-parcheo
- VM basada en pilas con una pila de operandos compartida y un marco por llamada activa
- Locales direccionados relativos a una base de marco, con parámetros como ranuras principales
- Recursión
- Cadenas y arreglos con conteo de referencias; los arreglos tienen semántica de referencia
- Estructuras y tuplas representadas como arreglos en tiempo de ejecución
- La VM nunca entra en pánico: cada fallo se manifiesta como un error tipado
- Errores en tiempo de ejecución para división por cero, `i64::MIN / -1` y `i64::MIN % -1`, índice de arreglo fuera de límites y agotamiento del límite de pasos
- Un presupuesto de 50.000.000 de pasos limita los bucles descontrolados para que fallen limpiamente en lugar de colgar
- Salida del programa capturada en una cadena, haciendo la ejecución determinista y testeable

### Verificador de bytecode

- Interpretación abstracta que rastrea la altura de la pila de operandos en lugar de valores concretos
- Demuestra que no hay desbordamiento hacia abajo de pila en ninguna instrucción
- Demuestra que dos flujos de control que llegan a la misma instrucción coinciden en la altura de la pila
- Demuestra que existen las ranuras locales, constantes de cadena, destinos de salto y destinos de llamada
- Demuestra que una llamada pasa exactamente tantos argumentos como declara la función llamada
- Demuestra que el control nunca cae al final, y que cada ruta termina en un `return`
- Lineal en el conteo de instrucciones
- Se ejecuta automáticamente antes de que se ejecute cualquier archivo objeto

### Artefactos de compilación anticipada

- `lumenc build` escribe un archivo objeto de bytecode textual orientado a líneas
- Línea de encabezado por función, una sección de constantes entrecomillada y escapada, una instrucción por línea
- La serialización y el análisis son inversos exactos, garantizados por una prueba de ida y vuelta
- `lumenc exec` carga, verifica y luego ejecuta un archivo objeto sin tocar la fuente

### IR de nivel medio

- Grafo de flujo de control de bloques básicos sobre registros virtuales con terminadores explícitos
- Terminadores `goto`, rama condicional y `return`
- Siete pases de flujo de datos que corren hasta un punto fijo: plegado de constantes, simplificación algebraica, propagación de copia, eliminación de subexpresiones comunes locales, eliminación de almacenamiento muerto, eliminación de código muerto y simplificación de CFG
- La simplificación de CFG descarta bloques inalcanzables, colapsa ramas cuyos brazos coinciden e hiliza bloques solo de `goto`
- Las instrucciones con efectos secundarios nunca son eliminadas por ningún pase
- Un intérprete MIR separado, probado diferencialmente contra la VM de pilas a través de 11 programas que cubren enteros, flotantes, cadenas, arreglos, estructuras, tuplas, recursión, bucles, operadores de cortocircuito y funciones integradas, requiriendo que ambos motores produzcan una salida idéntica
- Exportación Graphviz DOT del grafo de flujo de control

### Backend C

- Transpila el subconjunto escalar (`i64`, `f64`, `bool`, `unit`) a una unidad de traducción C99 autocontenida
- Funciones, recursión y todas las formas de flujo de control
- `if` en posición de valor aplanado a temporales explícitos
- Adición, sustracción y multiplicación de enteros emitidas a través de helpers de envoltura explícitos, coincidiendo con la semántica de la VM
- Declaraciones hacia adelante, para que las funciones puedan llamarse entre sí en cualquier orden
- Reporta un error claro para `str`, arreglos, estructuras y tuplas en lugar de emitir código incorrecto

### Herramientas y observabilidad

- `lumenc run`, `check`, `fmt`, `build`, `exec`, `dump` y `explain`
- Nueve formas de dump: `tokens`, `ast`, `hir`, `hir-opt`, `mir`, `cfg`, `c`, `bytecode`, `verify`
- `-O0` y `-O1`
- `--time` para tiempos por fase en microsegundos
- `-o` para la ruta de salida
- Formatador de fuente que emite Lumen válido y re-analizable; idempotente y de ida y vuelta, ambas propiedades probadas
- Desensamblador determinista que muestra índices de instrucción para que los destinos de salto sean legibles
- Spans `#[tracing::instrument]` en cada fase, filtrados a través de `RUST_LOG`
- Los logs van a stderr para que nunca se mezclen con la salida del programa en stdout
- Códigos de salida: `0` en éxito, `1` en error de compilación o tiempo de ejecución, `2` en error de uso o E/S

### Biblioteca estándar

- 39 funciones integradas
- Impresión para `i64`, `f64`, `bool` y `str`
- Matemáticas enteras: `abs`, `min`, `max`, `pow_int`, `gcd`, `lcm`, `sign`, `clamp`
- Matemáticas flotantes: `sqrt`, `abs_float`, `floor`, `ceil`, `round`, `pow_float`, `min_float`, `max_float`
- Conversiones numéricas: `to_float`, `to_int`
- Conversiones de cadena: `int_to_str`, `float_to_str`, `bool_to_str`, `char_to_str`, `parse_int`
- Consultas de cadena: `str_len`, `char_at`, `starts_with`, `ends_with`, `contains`, `index_of`
- Transformaciones de cadena: `substring`, `str_repeat`, `to_upper`, `to_lower`, `trim`
- Longitud de arreglo mediante `len`

### Pruebas y calidad

- 328 pruebas aprobadas: 212 pruebas unitarias en la biblioteca más 116 en 11 binarios de integración
- Pruebas de propiedades a través de `proptest`: la lexografía de entrada arbitraria nunca entra en pánico y siempre termina con exactamente un `Eof`; cada segmento de token está bien formado y dentro de los límites; el análisis de sopa de tokens arbitraria nunca entra en pánico
- Una suite de regresión dedicada para errores previamente corregidos
- Cada programa de ejemplo es verificado por la suite de pruebas
- Benchmarks de `criterion` por fase, más compilación y ejecución de extremo a extremo
- `cargo clippy --all-targets --all-features -- -D warnings` pasa limpio
- `cargo fmt --check` pasa limpio
- Cadena de herramientas fijada a Rust 1.96, edición 2024, para compilaciones reproducibles

## Aún no implementado

El alcance es intencionalmente incremental. Los siguientes son huecos conocidos, no omisiones:

- La adición, sustracción y multiplicación de enteros se envuelven silenciosamente en desbordamiento. No generan trampas. Solo `i64::MIN / -1` y `i64::MIN % -1` generan un error de desbordamiento.
- Los arreglos solo contienen `i64`, `f64`, `bool` o `str`. Se rechazan arreglos anidados, arreglos de estructuras y arreglos de tuplas.
- Los arreglos se construyen solo a partir de literales. No hay `push`, no hay `pop` y no hay forma de asignar un arreglo cuya longitud sea un valor en tiempo de ejecución.
- No hay entrada de ningún tipo. Ninguna función integrada lee stdin. Los programas son computación pura hacia stdout.
- Los patrones de `match` son literales escalares y el comodín. Sin vinculaciones, sin destrucción, sin rangos, sin patrones de "o".
- El backend C cubre solo el subconjunto escalar. `str`, arreglos, estructuras y tuplas son rechazados en lugar de transpilados.
- El backend C no reproduce la semántica de división de la VM. Emite `/` y `%` simples, por lo que una división por cero que la VM reporta como un error limpio en tiempo de ejecución se convierte en comportamiento indefinido en el C generado (en la práctica, `SIGFPE`).
- Una estructura recursiva como `struct Node { value: i64, next: Node }` pasa la verificación de tipos, aunque nunca se puede construir ningún valor de ella porque cada literal de estructura debe proporcionar todos los campos y no hay tipo nulo u opcional.
- El intérprete MIR es accesible solo desde pruebas. `lumenc` no tiene una bandera para ejecutar un programa en él.
- Sin genéricos, cierres, valores de función, enumeraciones, métodos o funciones anidadas.
- Un archivo fuente por invocación. No hay sistema de módulos o importación.

## Uso del compilador

Primero, compila el compilador:

```console
$ cargo build --release
```

Esto produce el binario `lumenc` en `target/release/lumenc`. Los comandos a continuación
lo escriben simplemente como `lumenc`, lo cual solo funciona si ese binario está en tu `PATH`.
Elige una de estas opciones:

- **Ejecútalo por ruta** (sin configuración): reemplaza `lumenc` con `./target/release/lumenc`,
  por ejemplo `./target/release/lumenc run examples/primes.lm`.
- **Ejecútalo vía cargo** (sin paso de compilación separado): coloca los argumentos después de `--`,
  por ejemplo `cargo run --release -- run examples/primes.lm`.
- **Instálalo** para que `lumenc` funcione directamente: `cargo install --path .` coloca `lumenc`
  en `~/.cargo/bin/` (en el `PATH` de Rust por defecto). Luego, los comandos a continuación funcionan
  literalmente. Verifícalo con `which lumenc`.

```console
$ lumenc run    examples/primes.lm     # compilar y ejecutar
$ lumenc check  examples/primes.lm     # solo verificar tipos
$ lumenc fmt    examples/primes.lm     # imprimir fuente canónicamente formateada
$ lumenc build  examples/fib.lm -o fib.lbc   # compilar a un objeto bytecode
$ lumenc exec   fib.lbc                # verificar y ejecutar un objeto compilado
$ lumenc explain E0318                 # explicar un código de diagnóstico

$ lumenc dump   ast      examples/fib.lm
$ lumenc dump   hir-opt  examples/fib.lm
$ lumenc dump   mir      examples/fib.lm
$ lumenc dump   bytecode examples/fib.lm
$ lumenc run    examples/fib.lm --time # tiempos por fase en stderr

$ RUST_LOG=lumen=debug lumenc run examples/fib.lm   # logs estructurados
```

Formas de dump: `tokens`, `ast`, `hir`, `hir-opt`, `mir`, `cfg`, `c`, `bytecode`,
`verify`. La optimización está activada por defecto; pasa `-O0` para desactivarla.

## Ejemplos

El directorio [`examples/`](examples) contiene programas ejecutables (`fib`,
`factorial`, `fizzbuzz` y `primes`), cada uno verificado por la suite de pruebas.

## Desarrollo

```console
$ cargo test                                   # unitarias + integración + regresión
$ cargo bench                                  # benchmarks criterion
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo fmt --check
```

El proyecto fija una cadena de herramientas de Rust mediante `rust-toolchain.toml` (Rust 1.96, edición
2024) para que las compilaciones sean reproducibles.

## Estructura del proyecto

```
src/
  span.rs, source.rs        spans and the line-indexed source map
  errors.rs, diagnostics.rs error codes and the diagnostics subsystem
  explain.rs, suggest.rs    error explanations and "did you mean" hints
  lexer/                    tokens and the scanner
  parser/                   AST, the parser, and an AST printer
  sema/                     types, name resolution, type checking
  hir/                      typed IR, lowering, and an HIR printer
  opt/                      the pass manager: inlining, folding, DCE
  mir/                      CFG-based mid-level IR, passes, interpreter
  backend/                  bytecode, codegen, VM, disassembler, verifier,
                            peephole optimizer, object format, C transpiler
  format.rs                 the source formatter
  session.rs                the pipeline driver
  main.rs                   the `lumenc` CLI
docs/                       design and language documentation
examples/                   sample programs
benches/                    criterion benchmarks
tests/                      integration, regression, and example tests
```

## Licencia

MIT. Ver [`LICENSE`](LICENSE).
