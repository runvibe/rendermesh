# Templates

RenderMesh supports Handlebars template rendering for HTML only.

## HTML-Only Rule

Files are compiled as templates when either condition is true:

- The object path ends with `.html` or `.htm`.
- The object metadata content type is `text/html` or compatible, such as `text/html; charset=utf-8`.

Non-HTML files are never compiled as templates.

## Compile Time

Templates are compiled after bucket sync:

- Once during startup sync.
- Again after every successful background sync for that origin.

Compiled templates are stored in an in-memory registry owned by the main runtime and shared through `Arc`.

## Render Time

RenderMesh renders a template only when an edge payload includes `params`.

No params:

```json
{
  "headers": {
    "x-edge": "static-pass-through"
  }
}
```

The file is served directly from the local mirror.

With params:

```json
{
  "params": {
    "title": "Hello"
  }
}
```

The resolved HTML file is rendered with Handlebars using the params.

## File Path With Params

```json
{
  "file_path": "/index.html",
  "params": {
    "title": "Hello"
  }
}
```

RenderMesh selects `/index.html` from the local mirror and renders the compiled template for that path.

Edge-selected `file_path` can select any mirrored origin file. If the selected file does not exist, RenderMesh executes the origin's configured missing-file behavior.

## Date Helpers

These Handlebars helpers use UTC and are evaluated when the HTML is rendered:

| Helper | Output |
| --- | --- |
| `{{date_now}}` | Current date and time in RFC 3339 with second precision, such as `2026-11-10T15:24:46Z`. |
| `{{date_today}}` | Current date as `YYYY-MM-DD`. |
| `{{date_time}}` | Current time as `HH:MM:SS`. |
| `{{date_timestamp}}` | Current Unix timestamp as an integer number of seconds. |
| `{{date_format value format}}` | A date or timestamp formatted with a Chrono `strftime` format string. |

`date_format` accepts an RFC 3339 string with a timezone offset, such as
`2026-11-10T12:30:00-03:00` or `2026-11-10T15:30:00Z`, or a JSON integer
containing Unix seconds. It converts the input instant to UTC before formatting.
The format uses [Chrono's strftime specifiers](https://docs.rs/chrono/0.4.44/chrono/format/strftime/).
For example, an edge hook can return:

```json
{
  "params": {
    "published_at": "2026-11-10T12:30:00-03:00",
    "updated_at": 0
  }
}
```

The HTML template can then use:

```handlebars
<p>Published: {{date_format published_at "%d/%m/%Y %H:%M"}} UTC</p>
<p>Updated: {{date_format updated_at "%Y-%m-%d"}}</p>
<p>Current time: {{date_format (date_now) "%H:%M:%S"}} UTC</p>
<p>Current date: {{date_format (date_timestamp) "%Y-%m-%d"}}</p>
```

The first two expressions render `10/11/2026 15:30` and `1970-01-01`.
The four current-time helpers take no arguments; `date_format` takes exactly
two positional arguments. Use `date_now` or `date_timestamp` as inputs to
`date_format`; the shorter `date_today` and `date_time` outputs are not complete
RFC 3339 timestamps. Invalid arguments, dates, timestamps, or format strings
cause a render error and return `502 Bad Gateway`.

Helper output uses normal Handlebars HTML escaping. Helper names take precedence
over same-named params in bare expressions; use an explicit path such as
`{{this.date_now}}` to access a param with that name. As with all templates,
an edge hook must return `params` for the HTML to be rendered; use
`{"params": {}}` when no other data is needed. A CDN or other cache can
continue serving a previously rendered value until it expires.

## Error Cases

- Params for a non-HTML file return `415 Unsupported Media Type`.
- Template render failures return `502 Bad Gateway`.
- Missing template entries are treated as unsupported HTML rendering for the request.

## Static HTML Without Params

HTML files are not automatically rendered. If the edge payload does not include `params`, the HTML is returned exactly as stored in the bucket.
