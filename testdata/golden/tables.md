# Tables

## Alignment

| Default | Left | Center | Right |
|---------|:-----|:------:|------:|
| a       | b    | c      | d     |
| longer cell | x | y | z |

## Inline content

| Markdown | Example |
|----------|---------|
| Emphasis | *em*, **strong**, ~~struck~~ |
| Code | `fmt.Println("\|")` |
| Unescaped pipe in code splits the cell, per the GFM spec | `a|b` |
| Escaped pipe | a \| b |
| Link | [example](https://example.com) |
| Image | ![dot](dot.png) |
| Emoji | :rocket: |
| HTML | <kbd>Ctrl</kbd> and <br> a break |
| Math | $x^2$ |

## Ragged rows

| A | B | C |
|---|---|---|
| one cell |
| two | cells |
| four | cells | here | extra |

## No outer pipes

Name | Value
--- | ---:
pi | 3.14
e | 2.72

## Header only

| Just | A | Header |
|------|---|--------|

## Empty cells

| A | B |
|---|---|
|   | b |
| a |   |

## Not tables

Missing the delimiter row:

| a | b |
| c | d |

Delimiter with wrong count:

| a | b |
|---|
| c | d |

A table right after a paragraph:
| x | y |
|---|---|
| 1 | 2 |

## Wide

| Column one | Column two | Column three | Column four | Column five | Column six | Column seven | Column eight |
|------------|------------|--------------|-------------|-------------|------------|--------------|--------------|
| Some long content that wraps | more | more | more | more | more | more | more |
