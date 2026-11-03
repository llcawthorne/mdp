# Math

## Inline

Euler: $e^{i\pi} + 1 = 0$. Bold: $\textbf{test}$. Blackboard: $\mathbb{R}^n$.

Underscores and stars stay TeX, not emphasis: $a_1 * b_2 * c_3$ and $x^*$.

Backtick form, for a $ inside: $`\text{cost} = \$5`$.

Math in a heading's text and in **bold $x$** and [a link $y$](https://example.com).

Escaped dollars stay text: \$5 and \$10. So do $5 and $10.

Two on a line: $a$ and $b$, adjacent $a$$b$.

## Display

$$
\begin{aligned}
  \nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \\
  \nabla \times \mathbf{B} &= \mu_0 \mathbf{J}
\end{aligned}
$$

$$ \sum_{k=1}^n k = \frac{n(n+1)}{2} $$

A matrix:

$$
A = \begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}
$$

```math
\int_{-\infty}^{\infty} e^{-x^2} \, dx = \sqrt{\pi}
```

```math
f(x) = \begin{cases} x & x \ge 0 \\ -x & x < 0 \end{cases}
```

## In other places

- A list item with $\alpha$ in it
- $$\beta$$ as display math in a list

> A quote with $\gamma$.

| Symbol | Meaning |
|--------|---------|
| $\pi$  | pi      |
| $\phi$ | phi     |

## Not math

In code: `$x$` and

```sh
echo "$HOME costs $5"
```

An empty pair $$ and a lone $ stay text.

## The $\LaTeX$ heading
