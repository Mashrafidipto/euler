1. - [x] make a small named binary el.
1. - [x] l open files in xdg-open.
1. - [x] Undo is not working properly . Program crashes after delete file op then renameing a file or folder. Then pressing undo undoes both op . And pasting it crashes the program again.
1. - [x] Try to keep the cursor in no 2 or 3 len . (Implemented: 4-column sliding viewport via `view_offset` keeps active cursor in column 2 or 3 when hiding 1st column and showing new child on the right; `Ctrl-h` / `Ctrl-l` move cursor across visible panes without changing the layout)
1. - [x] Make the cursor more noticabel . Don't highlight the opened folder deeply . A simple highlight will do . 
1. - [x] I changed my mind . l will open in Editor as previously done and enter will open file in xdg-open. 
1. - [x] The below code falls back to vim . Don't do that .Try to onen in xdg-open if no other option is found . 
```rs
    fn default() -> Self {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "vim".to_string());
            ....
    }

```
1. - [x] Show an Error if xdg-open fails.
