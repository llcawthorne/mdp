# Mermaid Diagrams

## Flowchart

```mermaid
flowchart TD
  A[Start] --> B{Is it working?}
  B -->|Yes| C[Ship it]
  B -->|No| D["Fix <it> & retry"]
  D --> B
```

## Sequence

```mermaid
sequenceDiagram
  participant U as User
  participant M as mdp
  U->>M: mdp -w notes.md
  M-->>U: http://127.0.0.1:1234/
  loop every 250ms
    M->>M: check notes.md
  end
```

## Class diagram

```mermaid
classDiagram
  class watcher {
    +string filename
    +serveEvents()
  }
```

## State, Gantt, and pie

```mermaid
stateDiagram-v2
  [*] --> Draft
  Draft --> Published
  Published --> [*]
```

```mermaid
gantt
  title Reading plan
  dateFormat YYYY-MM-DD
  section Rust
  The Book :2026-10-01, 30d
```

```mermaid
pie title Languages
  "Go" : 40
  "Rust" : 60
```

## Not diagrams

A `mermaid` word in code, and a block in another language:

```text
graph LR
  A --> B
```

In a list:

1. Step one:

   ```mermaid
   graph LR
     X --> Y
   ```
