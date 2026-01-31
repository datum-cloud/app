# Icons

Place SVG files here to use them in the app via the `Icon` component or `Button { leading_icon: ... }`.

- **Naming**: Use the file name without `.svg` as the icon name (e.g. `plus.svg` → `"plus"`).
- **Styling**: Use `stroke="currentColor"` and/or `fill="currentColor"` in your SVGs so they inherit text/button color.
- **Registration**: Add a match arm in `ui/src/components/icon.rs` in `svg_content_for()` so the icon is included at compile time:

  ```rust
  "your-icon-name" => Some(include_str!("../../assets/icons/your-icon-name.svg")),
  ```

Then use in buttons:

```rust
Button {
    leading_icon: Some(IconSource::Named("plus".into())),
    text: "Add".to_string(),
    ...
}
```

Or standalone:

```rust
Icon { source: IconSource::Named("check".into()), size: 24 }
```
