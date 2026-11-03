# GitHub Flavored Markdown

## Tables

| Left | Center | Right | Inline |
|:-----|:------:|------:|--------|
| a    | b      | c     | `code` and **bold** |
| long cell content | | 3.14 | escaped \| pipe |

Not a table, no delimiter row:
| a | b |

## Lists

1. First
2. Second
   - nested bullet
     with a lazy continuation
   - another

     with a second paragraph
3. Third, containing code:

   ```go
   fmt.Println("in a list")
   ```

5. Starts a list at 5? No, continues the list.

- [x] done task
- [ ] open task
  - [x] nested done task
- not a task [ ]

* star bullet
+ plus bullet starts a new list

## Quotes

> Quote with a list:
>
> - one
> - two
>
> > Nested quote with `code`.

## Inline

~~strikethrough~~, *em*, _em_, **strong**, __strong__, ***both***,
intra_word_underscores stay, `` code with ` backtick ``, and
a hard break with two spaces  
then a hard break with a backslash\
then done.

Autolinks: https://example.com, www.example.com, <https://example.org>,
and someone@example.com.

Entities: &copy; &amp; &#x1F600; &lt;not a tag&gt;. Escapes: \*not em\*,
\[not a link\], 1\. not a list.

---
***
___

Reference [link][ref] and [collapsed][] and [shortcut].

[ref]: https://example.com/ref "Ref Title"
[collapsed]: https://example.com/collapsed
[shortcut]: https://example.com/shortcut
