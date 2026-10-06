import { useState } from 'react'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'

/**
 * A JSON value edited as text. Emits the parsed value while the text parses; otherwise says so
 * and keeps the last good value. `describe` shows the value in plain words under the editor.
 */
export function JsonField({
  label,
  value,
  onChange,
  describe,
  minHeight = 48,
  maxHeight = 220,
  placeholder,
  optional = false,
}: {
  label: string
  value: unknown
  onChange: (v: unknown) => void
  describe?: (v: unknown) => string
  minHeight?: number
  maxHeight?: number
  placeholder?: string
  /** May be left empty: edited without the JSON linter, which flags an empty document. */
  optional?: boolean
}) {
  const [text, setText] = useState(() => (value === undefined ? '' : JSON.stringify(value, null, 2)))
  const [bad, setBad] = useState(false)
  return (
    <div className="stack" style={{ gap: 4 }}>
      <CodeEditor
        aria-label={label}
        language={optional ? 'text' : 'json'}
        value={text}
        minHeight={minHeight}
        maxHeight={maxHeight}
        placeholder={placeholder}
        onChange={(t) => {
          setText(t)
          if (t.trim() === '') {
            setBad(false)
            onChange(undefined)
            return
          }
          try {
            onChange(JSON.parse(t))
            setBad(false)
          } catch {
            setBad(true)
          }
        }}
      />
      {bad ? (
        <span className="error-text">Not valid JSON yet; the last valid value is kept.</span>
      ) : (
        describe && value !== undefined && <span className="muted">{describe(value)}</span>
      )}
    </div>
  )
}
