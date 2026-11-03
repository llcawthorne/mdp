---
title: GitHub Additions
tags: [markdown, github]
author:
  name: Lewis
  editor: neovim
---

# GitHub Additions

## Footnotes

A claim[^1] and another.[^note]

[^1]: The first footnote.
[^note]: A named footnote, with `code`.

## Alerts

> [!NOTE]
> Useful information.

> [!tip]
> Lowercase markers work too.

> [!WARNING]
> **Bold** warning
> over two lines.

> [!CAUTION]

> [!NOPE]
> Unknown types stay plain quotes.

> [!NOTE] Text on the marker line
> keeps it a plain quote.

## Emoji

:rocket: :+1: :tada: and :not-an-emoji:.

## Math

Inline $e^{i\pi} + 1 = 0$, with backticks $`\sqrt{2}`$, and display:

$$
\sum_{n=1}^\infty \frac{1}{n^2} = \frac{\pi^2}{6}
$$

Prices like $5 and $10 stay text, as do $ spaced $ and escaped \$x\$.
HTML in math is escaped: $a<b$ and $x > y$.

```math
\int_0^1 x^2 \, dx = \frac{1}{3}
```

## Mermaid

```mermaid
graph LR
  A[Markdown] --> B{mdp} --> C(HTML)
```

## HTML GitHub allows

<p align="center"><img src="dot.png" width="40" alt="centered local dot"></p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="dot.png">
  <img src="dot.png" alt="picture fallback">
</picture>

![dark only](dot.png#gh-dark-mode-only)
![light only](dot.png#gh-light-mode-only)

## Links to files

[Another note](other.md#a-section), [a folder](docs/), and [a page
anchor](#footnotes).
