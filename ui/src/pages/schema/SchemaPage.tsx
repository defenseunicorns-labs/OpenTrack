import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TbEye, TbPencil, TbPlus, TbSend, TbTrash } from 'react-icons/tb'
import { Badge, Button, DataTable, SaveButton, TabPanel, Tabs, useToast } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { jsonParseLinter } from '@codemirror/lang-json'
import { ensureSyntaxTree, syntaxTree } from '@codemirror/language'
import { EditorView } from '@codemirror/view'
import { api, type ExtensionField, type SchemaOverview, type SourceRow } from '../../api/client'
import { errorMessage } from '../../lib/format'
import { unpublished, type Unpublished } from '../../lib/schemaUsage'
import { FieldForm } from './FieldForm'
import { SchemaFieldTable } from './SchemaFieldTable'
import { SchemaOutputPreview, SchemaPublishReview } from './SchemaReview'
import { SchemaVersionSelect } from './SchemaVersionSelect'
import { BACKGROUND_TABLE, FIELD_TYPES, compareFields, sameFields, schemaFields, type SchemaChange } from './schemaFields'
import { InfoTip } from '../../components/InfoTip'
import { useCan } from '../../auth/context'

const MODES = [
  { id: 'form', label: 'Form' },
  { id: 'json', label: 'JSON' },
  { id: 'fields', label: 'Fields' },
]

const asJson = (f: ExtensionField[]) => JSON.stringify(f, null, 2)

type DraftJson = { fields: ExtensionField[] } | { message: string; index?: number }

function readDraftJson(text: string): DraftJson {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return { message: 'Fix the JSON before continuing.' }
  }
  if (!Array.isArray(value)) return { message: 'The draft must be a JSON array of fields.' }
  const index = value.findIndex((field) => (
    !field || typeof field !== 'object' || Array.isArray(field) ||
    typeof field.key !== 'string' || !FIELD_TYPES.includes(field.type) ||
    ['unit', 'description', 'builtin'].some((key) => field[key] != null && typeof field[key] !== 'string') ||
    (field.enum_values != null && (!Array.isArray(field.enum_values) || field.enum_values.some((item: unknown) => typeof item !== 'string')))
  ))
  return index >= 0
    ? { message: `Check the name, type, and values for field ${index + 1}.`, index }
    : { fields: value }
}

export default function SchemaPage({ onDirtyChange, visible }: { onDirtyChange: (dirty: boolean) => void; visible: boolean }) {
  const { toast, confirm } = useToast()
  const canAdmin = useCan('admin')
  const [overview, setOverview] = useState<SchemaOverview | null>(null)
  const [sources, setSources] = useState<SourceRow[]>([])
  const [selected, setSelected] = useState<number | null>(null)
  // The draft being edited (null when not editing), as fields and as JSON text.
  const [fields, setFields] = useState<ExtensionField[] | null>(null)
  const [mode, setMode] = useState('form')
  const [text, setText] = useState('')
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [previewFields, setPreviewFields] = useState<ExtensionField[] | null>(null)
  const [review, setReview] = useState<{ version: number; live: number | null; fields: ExtensionField[]; changes: SchemaChange[] } | null>(null)
  const [reviewError, setReviewError] = useState<string | null>(null)
  const [publishing, setPublishing] = useState(false)
  const editor = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    const [o, s] = await Promise.all([api.schema(), api.sources()])
    setOverview(o)
    setSources(s)
    return { overview: o, sources: s }
  }, [])

  useEffect(() => {
    // Fetch on mount: an external system (the API), which is what effects are for.
    // eslint-disable-next-line react/set-state-in-effect
    load().catch((e) => setError(errorMessage(e)))
  }, [load])

  const versions = overview?.versions ?? []
  const draft = versions.find((v) => v.status === 'draft')
  const latestPublished = versions.find((v) => v.version === overview?.latest_published)
  const draftNumber = draft?.version ?? (latestPublished ? latestPublished.version + 1 : 1)
  const editing = fields !== null
  const versionOptions = editing && !draft
    ? [{ version: draftNumber, status: 'draft' as const }, ...versions.toReversed()]
    : versions.toReversed()
  const shownVersion = versionOptions.find((v) => v.version === selected)?.version ?? versionOptions[0]?.version ?? null
  const current = versions.find((v) => v.version === shownVersion) ?? null
  const showEditor = editing && shownVersion === draftNumber

  const currentFields = useMemo(() => overview ? schemaFields(overview, current?.fields ?? [], sources) : [], [overview, current, sources])
  const draftRows = useMemo(() => overview && fields ? schemaFields(overview, fields, sources) : [], [overview, fields, sources])
  const usage = useMemo(() =>
    Object.fromEntries(draftRows.filter((row) => !row.core).map((row) => (
      [row.key, [...new Set(row.inputs.map(({ source }) => source))]]
    ))), [draftRows])
  const missing = useMemo(() => (overview ? unpublished(sources, overview) : []), [overview, sources])

  const baseFields = draft?.fields ?? latestPublished?.fields ?? []
  const parsed = editing && mode === 'json' ? readDraftJson(text) : null
  const dirty = editing && (parsed ? !('fields' in parsed) || !sameFields(parsed.fields, baseFields) : !sameFields(fields, baseFields))
  useEffect(() => { onDirtyChange(dirty) }, [dirty, onDirtyChange])
  useEffect(() => {
    if (!dirty) return
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = '' }
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [dirty])

  const setBoth = (next: ExtensionField[]) => {
    setFields(next)
    setText(asJson(next))
    setError(null)
  }
  const startEditing = () => {
    if (!editing) {
      setBoth(baseFields)
      setMode('form')
    }
    setSelected(draftNumber)
  }
  const cancel = async () => {
    if (dirty && !await confirm('Close the editor and lose the changes you have not saved?', { title: 'Cancel', confirmLabel: 'Cancel editing' })) return
    setFields(null)
    setError(null)
  }

  const edited = (): ExtensionField[] | null => {
    if (!parsed) return fields ?? baseFields
    if ('fields' in parsed) return parsed.fields
    const root = editor.current?.querySelector<HTMLElement>('.cm-editor')
    const view = root && EditorView.findFromDOM(root)
    let message = parsed.message
    if (view) {
      const tree = ensureSyntaxTree(view.state, view.state.doc.length, 50) ?? syntaxTree(view.state)
      let errorOffset: number | undefined
      tree.iterate({ enter: (node) => {
        if (node.type.isError) errorOffset ??= node.from
        return errorOffset === undefined
      } })
      const array = tree.topNode.firstChild
      let entry = array?.firstChild ?? null
      for (let index = 0; entry && parsed.index !== undefined; entry = entry.nextSibling) {
        if (entry.name === '[' || entry.name === ']' || entry.name === ',') continue
        if (index++ === parsed.index) break
      }
      const position = errorOffset ?? jsonParseLinter()(view)[0]?.from ?? (parsed.index !== undefined ? entry?.from : undefined) ?? 0
      const line = view.state.doc.lineAt(position).number
      message = `Line ${line}. ${message}`
      requestAnimationFrame(() => {
        view.dispatch({ selection: { anchor: position }, scrollIntoView: true })
        root.scrollIntoView({ block: 'nearest' })
        view.focus()
      })
    }
    setError(message)
    return null
  }

  const switchMode = (m: string) => {
    if (m === mode) return
    if (mode === 'json') {
      const next = edited()
      if (next === null) return
      setFields(next)
    } else if (m === 'json' && fields) {
      setText(asJson(fields))
    }
    setError(null)
    setMode(m)
  }

  const openPreview = () => {
    const next = edited()
    if (next === null) return
    setError(null)
    setPreviewFields(next)
  }

  const openReview = async () => {
    if (!draft || dirty) return
    setError(null)
    try {
      const { overview: fresh, sources: freshSources } = await load()
      const savedDraft = fresh.versions.find((v) => v.status === 'draft')
      if (!savedDraft || savedDraft.version !== draft.version || !sameFields(savedDraft.fields, draft.fields)) {
        setError('The saved draft changed. Close the editor and reopen the draft to review it.')
        return
      }
      const current = fresh.versions.find((v) => v.version === fresh.latest_published)
      setReview({
        version: savedDraft.version, live: fresh.latest_published, fields: savedDraft.fields,
        changes: compareFields(schemaFields(fresh, current?.fields ?? [], freshSources), schemaFields(fresh, savedDraft.fields, freshSources)),
      })
      setReviewError(null)
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const saveDraft = async () => {
    const f = edited()
    if (f === null) return
    setSaving(true)
    setError(null)
    try {
      const d = await api.saveDraft(f, draft?.notes ?? undefined)
      await load()
      setBoth(d.fields)
      setSelected(d.version)
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const publish = async () => {
    if (!review || publishing) return
    setPublishing(true)
    setReviewError(null)
    try {
      const fresh = await api.schema()
      const savedDraft = fresh.versions.find((version) => version.status === 'draft')
      if (savedDraft?.version !== review.version || fresh.latest_published !== review.live || !sameFields(savedDraft.fields, review.fields)) {
        setReviewError('The schema changed while this review was open. Close it and review the saved draft again.')
        return
      }
      const p = await api.publishDraft()
      setReview(null)
      setFields(null)
      await load()
      setSelected(p.version)
      toast({ variant: 'success', message: `Schema version ${p.version} published.` })
    } catch (e) {
      setReviewError(errorMessage(e))
    } finally {
      setPublishing(false)
    }
  }

  const discard = async () => {
    const ok = await confirm(`Discard draft version ${draft?.version}? Its changes are lost.`, { title: 'Discard draft', confirmLabel: 'Discard' })
    if (!ok) return
    try {
      await api.discardDraft()
      setFields(null)
      const { overview: o } = await load()
      setSelected(o.latest_published)
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const addMissing = (u: Unpublished) => {
    if (editing) setSelected(draftNumber)
    const next = edited()
    if (next === null) return
    if (next.some((f) => f.key === u.suggestion.key)) {
      toast({ variant: 'warning', message: `The draft already has a field named ${u.suggestion.key}; rename one of them.` })
    }
    setBoth([...next, u.suggestion])
    setSelected(draftNumber)
    if (mode !== 'json' || !editing) setMode('form')
    requestAnimationFrame(() => {
      const input = editor.current?.querySelector<HTMLInputElement>(`input[aria-label="Field ${next.length + 1} name"]`)
      if (input) input.focus()
      else editor.current?.scrollIntoView({ block: 'nearest' })
    })
  }
  const fieldToolbarStart = (
    <>
      <SchemaVersionSelect versions={versionOptions} selected={shownVersion} disabled={saving} onSelect={setSelected} />
      <InfoTip label="Fields">
        {showEditor ? (
          <>
            {draft ? 'Save keeps your changes in the draft.' : `Save creates draft version ${draftNumber}.`}
            {' '}Publish makes it the live version and locks its fields. OpenTrack sends a fresh update for live tracks whose fields changed.
          </>
        ) : (
          <>
            Choose a version to see its fields. OpenTrack sends the extra fields from the newest published version at <span className="mono">attributes.&lt;field&gt;</span>.
            A star marks the fields it always sends. Select a field to see where its value comes from.
          </>
        )}
      </InfoTip>
    </>
  )

  return (
    <div className="panels">
      <section className="schema-section" aria-label="Fields">
        {error && <div className="error-text" role="alert">{error}</div>}
        {editing && overview && (
          <div hidden={!showEditor}>
            <div className="toolbar schema-section-toolbar">
              {mode !== 'fields' && fieldToolbarStart}
              <Tabs aria-label="Draft views" idPrefix="schema" size="sm" value={mode} onChange={switchMode} tabs={MODES} />
              <span className="spacer" />
              <Button size="sm" variant="ghost" icon={<TbEye />} disabled={saving} onClick={openPreview}>Preview output</Button>
              <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={saveDraft} />
              {draft && (
                <Button size="sm" variant="secondary" icon={<TbSend />} onClick={() => void openReview()} disabled={dirty || saving}>
                  Publish
                </Button>
              )}
              {draft && (
                <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label="Discard draft" title="Discard draft" onClick={discard} />
              )}
              <Button size="sm" variant="ghost" onClick={() => void cancel()} disabled={saving}>Cancel</Button>
            </div>
            <div className={mode === 'fields' ? undefined : 'schema-editor'} ref={editor} inert={saving}>
              <TabPanel id={mode} idPrefix="schema">
                {mode === 'form' && <FieldForm fields={fields} onChange={setFields} schema={overview} usage={usage} />}
                {mode === 'json' && <CodeEditor aria-label="Draft fields" value={text} onChange={(value) => { setText(value); setError(null) }} minHeight={240} maxHeight={560} />}
                {mode === 'fields' && (
                  <SchemaFieldTable rows={draftRows} loaded visible={visible && showEditor} toolbarStart={fieldToolbarStart} />
                )}
              </TabPanel>
            </div>
          </div>
        )}
        {!showEditor && (
          <SchemaFieldTable
            key={shownVersion}
            rows={currentFields}
            loaded={!!overview}
            visible={visible}
            toolbarStart={fieldToolbarStart}
            actions={
              <Button size="sm" icon={<TbPencil />} disabled={!canAdmin || !overview} onClick={startEditing}>
                {draft || editing ? 'Edit draft' : 'New draft'}
              </Button>
            }
          />
        )}
      </section>

      <section className="schema-section" aria-labelledby="schema-unpublished-heading">
        <div className="toolbar schema-section-toolbar">
          <h2 className="subhead" id="schema-unpublished-heading">Set by feeds, not published</h2>
          <InfoTip label="Set by feeds, not published">
            These values come from enabled feeds, but OpenTrack leaves them out of {latestPublished ? `published version ${latestPublished.version}` : 'the published output'}.
            Select Add to put a field in the draft. OpenTrack starts sending it after you save and publish the draft.
          </InfoTip>
          {missing.length > 0 && <Badge color="grey" size="sm">{missing.length}</Badge>}
        </div>
        <DataTable
          aria-label="Values not published"
          columns={[
            { key: 'target', header: 'OpenTrack field', mono: true, width: '24%', render: (u) => u.target, sortValue: (u) => u.target },
            { key: 'from', header: 'Set by', mono: true, render: (u) => u.from.join(' · ') },
            {
              key: 'as',
              header: 'Would publish as',
              width: '26%',
              render: (u) => (
                <span className="mono">
                  attributes.{u.suggestion.key}
                  {u.suggestion.builtin && <span className="muted"> (built-in {u.suggestion.builtin})</span>}
                </span>
              ),
            },
            {
              key: 'add',
              header: '',
              width: 110,
              align: 'right',
              render: (u) => (
                <Button size="sm" variant="secondary" icon={<TbPlus />} disabled={!canAdmin || saving} onClick={() => addMissing(u)}>
                  Add
                </Button>
              ),
            },
          ]}
          rows={missing}
          rowKey={(u) => u.target}
          empty={overview ? 'Everything the enabled sources set is published.' : 'LOADING…'}
          style={BACKGROUND_TABLE}
        />
      </section>

      {visible && previewFields && <SchemaOutputPreview fields={previewFields} onClose={() => setPreviewFields(null)} />}
      {visible && review && (
        <SchemaPublishReview
          version={review.version}
          changes={review.changes}
          publishing={publishing}
          error={reviewError}
          onPublish={() => void publish()}
          onClose={() => { if (!publishing) setReview(null) }}
        />
      )}
    </div>
  )
}
