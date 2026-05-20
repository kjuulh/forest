/*!
 * highlight.js language definition for CUE (cuelang.org).
 *
 * CUE is not part of highlight.js's bundled language set and no
 * actively maintained third-party package exists, so we ship a
 * hand-written grammar. Covers:
 *   - line / block comments
 *   - strings with `\(expr)` interpolation, plus the `#"..."#` and
 *     `##"..."##` raw variants with their corresponding interpolation
 *     forms (`\#(...)`, `\##(...)`)
 *   - byte literals `'...'` and `'''...'''`
 *   - numbers with hex/oct/bin/dec, scientific notation, and CUE's
 *     IEC/SI suffixes (`Ki`, `M`, `Gi`, etc.)
 *   - definitions `#Foo` and hidden definitions `_#Foo`
 *   - hidden fields `_field:` and optional/required markers
 *     (`?:`, `!:`)
 *   - operators (`&`, `|`, `=~`, `!~`, `..`, `*`, comparisons)
 *   - attributes `@name(args)`
 *   - the bottom value `_|_`
 *
 * Tree-sitter-grade accuracy isn't possible inside highlight.js's
 * regex-driven model — anything beyond this falls back to plain
 * identifier text. See `specs/reviews/010-adversarial-review.md` for
 * why we considered (and didn't ship) a server-side tree-sitter
 * highlighter.
 *
 * Registers under the name `cue`, so markdown fences like
 *   ```cue
 *   ```
 * are picked up automatically by `hljs.highlightElement`.
 */
(function () {
    function cue(hljs) {
        var KEYWORDS = {
            keyword: 'package import for in if let',
            literal: 'true false null',
            type:
                'int string bool number bytes float uint rune _ ' +
                'int8 int16 int32 int64 int128 ' +
                'uint8 uint16 uint32 uint64 uint128 ' +
                'float32 float64',
            built_in: 'len close or and div mod quo rem'
        };

        // Embedded expression inside a string. CUE matches the
        // interpolation marker to the raw-string hash count, so a
        // plain `"..."` uses `\(...)`, a `#"..."#` uses `\#(...)`, etc.
        // We provide separate variants instead of trying to count
        // hashes — highlight.js can't do dynamic delimiter matching.
        function interpolation(beginRe) {
            return {
                className: 'subst',
                begin: beginRe,
                end: /\)/,
                keywords: KEYWORDS
                // Nested strings inside interpolations render as plain
                // text — a deliberate trade-off for grammar simplicity.
            };
        }

        var STRING = {
            className: 'string',
            variants: [
                // Triple-quoted multi-line: """ ... """
                {
                    begin: /"""/, end: /"""/,
                    contains: [
                        hljs.BACKSLASH_ESCAPE,
                        interpolation(/\\\(/)
                    ]
                },
                // Double-hash raw: ##" ... "##   (interp: \##(...))
                {
                    begin: /##"/, end: /"##/,
                    contains: [interpolation(/\\##\(/)]
                },
                // Single-hash raw: #" ... "#    (interp: \#(...))
                {
                    begin: /#"/, end: /"#/,
                    contains: [interpolation(/\\#\(/)]
                },
                // Regular: " ... "
                {
                    begin: /"/, end: /"/,
                    contains: [
                        hljs.BACKSLASH_ESCAPE,
                        interpolation(/\\\(/)
                    ],
                    illegal: '\\n'
                }
            ]
        };

        // CUE byte literals. Distinct from rune literals (which CUE
        // doesn't have — Go does). `''' ... '''` is multi-line, `'...'`
        // single-line. Same `\(...)` interpolation rules as strings.
        var BYTES = {
            className: 'string',
            variants: [
                {
                    begin: /'''/, end: /'''/,
                    contains: [
                        hljs.BACKSLASH_ESCAPE,
                        interpolation(/\\\(/)
                    ]
                },
                {
                    begin: /'/, end: /'/,
                    contains: [
                        hljs.BACKSLASH_ESCAPE,
                        interpolation(/\\\(/)
                    ],
                    illegal: '\\n'
                }
            ]
        };

        // Numbers: integer / float with optional CUE multiplier suffix
        // (K, Ki, M, Mi, G, Gi, T, Ti, P, Pi). Hex/oct/bin matched
        // before decimal so the `0x`/`0o`/`0b` prefixes don't get
        // eaten by the decimal pattern.
        var NUMBER = {
            className: 'number',
            variants: [
                { begin: /\b0x[0-9a-fA-F_]+/ },
                { begin: /\b0o[0-7_]+/ },
                { begin: /\b0b[01_]+/ },
                {
                    begin:
                        /\b\d[\d_]*(\.[\d_]+)?([eE][+-]?\d+)?([KMGTP]i?)?\b/
                }
            ],
            relevance: 0
        };

        // CUE definitions: `#Foo`, `_#Foo`, optionally followed by
        // chained selectors like `#Foo.#Bar`. We match the first
        // sharp-prefixed component; subsequent ones get matched on
        // their own thanks to highlight.js's continuous scanning.
        var DEFINITION = {
            className: 'title.class',
            begin: /_?#[A-Za-z_][A-Za-z0-9_]*/
        };

        // The bottom value (`_|_`). Match before `OPERATOR` so the
        // `|` in the middle doesn't get coloured as an operator.
        var BOTTOM = {
            className: 'literal',
            begin: /_\|_/
        };

        // Field labels at line start. Patterns covered:
        //   foo:           regular
        //   foo?:          optional
        //   foo!:          required
        //   _foo:          hidden
        //   _foo?:         hidden + optional
        //   foo-bar:       kebab-case (CUE allows hyphens in labels)
        //
        // Quoted labels like "foo bar": render as a normal string
        // followed by a colon — the string mode handles them.
        var FIELD = {
            className: 'attr',
            begin: /^\s*_?[A-Za-z_][A-Za-z0-9_-]*\s*[?!]?:/,
            relevance: 0
        };

        // Operators that carry meaning in CUE. `?` is intentionally
        // omitted — it only appears as `?:` on optional fields and
        // would otherwise collide with FIELD.
        var OPERATOR = {
            className: 'operator',
            begin: /=~|!~|<=|>=|==|!=|\.\.|&&|\|\||[&|*<>]/,
            relevance: 0
        };

        // `@attr(args)`. The contents are arbitrary — let strings,
        // numbers, and keywords highlight inside.
        var ATTRIBUTE = {
            className: 'meta',
            begin: /@[A-Za-z_][A-Za-z0-9_]*\(/,
            end: /\)/,
            keywords: KEYWORDS,
            contains: [STRING, NUMBER]
        };

        return {
            name: 'CUE',
            aliases: ['cue'],
            keywords: KEYWORDS,
            contains: [
                hljs.C_LINE_COMMENT_MODE,
                hljs.C_BLOCK_COMMENT_MODE,
                STRING,
                BYTES,
                NUMBER,
                ATTRIBUTE,
                BOTTOM,
                DEFINITION,
                FIELD,
                OPERATOR
            ]
        };
    }

    if (typeof hljs !== 'undefined') {
        hljs.registerLanguage('cue', cue);
    }
})();
