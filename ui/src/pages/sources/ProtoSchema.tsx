import { useEffect, useState } from 'react'
import { TbTrash } from 'react-icons/tb'
import { Badge, Button, FieldSelect, FileDropZone, Label, useToast } from '@kineticlogic/staresdk'
import { api, type ProtoDescription, type SourceSpec } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage } from '../../lib/format'

type Codec = SourceSpec['pipeline']['codec']

/** Largest .proto file taken (the server allows 2 MB in all). */
const MAX_FILE = 512 * 1024

/**
 * The producer's .proto files for a protobuf codec: add them one at a time (a file that imports another needs it
 * too), see whether they compile, and pick the message each frame holds. Reports what they define to the parent,
 * which offers the gRPC methods.
 */
export function ProtoSchema({
  codec,
  onCodec,
  onDescription,
}: {
  codec: Codec
  onCodec: (c: Codec) => void
  onDescription: (d: ProtoDescription | null) => void
}) {
  const { toast } = useToast()
  const files = (codec.files as Record<string, string> | undefined) ?? {}
  const names = Object.keys(files).sort()
  const [picked, setPicked] = useState<File | null>(null)
  const [described, setDescribed] = useState<ProtoDescription | null>(null)
  const [error, setError] = useState<string | null>(null)
  const key = JSON.stringify(files)

  useEffect(() => {
    if (names.length === 0) {
      setDescribed(null)
      setError(null)
      onDescription(null)
      return
    }
    let live = true
    const t = setTimeout(() => {
      api.describeProtobuf(files).then(
        (d) => {
          if (!live) return
          setDescribed(d)
          setError(null)
          onDescription(d)
        },
        (e) => {
          if (!live) return
          setDescribed(null)
          setError(errorMessage(e))
          onDescription(null)
        },
      )
    }, 300)
    return () => {
      live = false
      clearTimeout(t)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  const add = async (file: File | null) => {
    setPicked(null)
    if (!file) return
    if (file.size > MAX_FILE) {
      toast({ variant: 'error', title: '.proto', message: `${file.name} is over 512 KB` })
      return
    }
    const text = await file.text()
    onCodec({ ...codec, files: { ...files, [file.name]: text } })
  }
  const remove = (name: string) => {
    const next = { ...files }
    delete next[name]
    onCodec({ ...codec, files: next })
  }

  const messages = described?.messages ?? []
  const message = messages.find((m) => m.name === codec.message)
  // Repeated message fields of the chosen message: where the records are.
  const lists = (message?.fields ?? []).filter((f) => f.repeated && f.type.startsWith('message '))

  return (
    <div className="field wide stack" style={{ gap: 8 }}>
      <div className="num-row">
        <Label size="sm">.proto files</Label>
        <InfoTip label=".proto files">
          The producer&apos;s schema, compiled here: no protoc, no rebuild. Add every file the producer gives you; they may import each other
          by the names shown, and the google/protobuf well-known types are built in. Records follow the proto3 JSON mapping with the
          .proto&apos;s own field names, 64-bit integers as numbers and fields at their default included (a latitude of 0 is 0).
        </InfoTip>
        {names.length > 0 &&
          (error ? (
            <Badge color="danger" size="sm">
              does not compile
            </Badge>
          ) : described ? (
            <Badge color="success" size="sm">
              {described.messages.length} messages, {described.methods.length} methods
            </Badge>
          ) : null)}
      </div>
      {names.map((n) => (
        <div key={n} className="num-row">
          <span className="mono">{n}</span>
          <span className="muted">{Math.ceil(files[n].length / 1024)} KB</span>
          <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label={`Remove ${n}`} onClick={() => remove(n)} />
        </div>
      ))}
      <FileDropZone
        inputId="proto-file"
        accept=".proto"
        acceptedExtensions={['.proto']}
        file={picked}
        onFileChange={add}
        onReject={(m) => toast({ variant: 'error', title: '.proto', message: m })}
        hint="One .proto at a time; add its imports too."
        compact
      />
      {error && <pre className="notice mono" style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{error}</pre>}
      {messages.length > 0 && (
        <div className="form-grid">
          <div className="field">
            <div className="row-label">
              <Label size="sm">Message</Label>
              <InfoTip label="Message">
                The message type each frame holds, as declared in the .proto files. With gRPC it must match the methods: what a called method returns, or what producers send to OpenTrack.
              </InfoTip>
            </div>
            <FieldSelect
              ariaLabel="Message"
              fields={messages.map((m) => ({ name: m.name }))}
              value={(codec.message as string) || null}
              onChange={(name) => {
                if (!name) return
                const next: Codec = { ...codec, message: name }
                delete next.records
                onCodec(next)
              }}
            />
          </div>
          {lists.length > 0 && (
            <div className="field">
              <div className="row-label">
                <Label size="sm">Records</Label>
                <InfoTip label="Records">
                  A repeated field of the message whose elements are one record each. The whole message: each frame is one record.
                </InfoTip>
              </div>
              <FieldSelect
                ariaLabel="Records"
                fields={[{ name: '(the whole message)' }, ...lists.map((f) => ({ name: f.name }))]}
                value={(codec.records as string) || '(the whole message)'}
                onChange={(name) => {
                  const next: Codec = { ...codec }
                  if (!name || name === '(the whole message)') delete next.records
                  else next.records = name
                  onCodec(next)
                }}
              />
            </div>
          )}
        </div>
      )}
    </div>
  )
}
