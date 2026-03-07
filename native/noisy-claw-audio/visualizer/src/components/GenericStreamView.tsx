import type { AudioFrame, MetadataEvent, DataStreamDescriptor } from '../lib/protocol'
import { WaveformCanvas } from './WaveformCanvas'
import { SpectrumCanvas } from './SpectrumCanvas'
import { SpectrogramCanvas } from './SpectrogramCanvas'
import { MetadataStreamView } from './MetadataStreamView'
import { TextStreamView } from './TextStreamView'
import { getTapColor } from '../lib/colors'

interface GenericStreamViewProps {
  descriptor: DataStreamDescriptor
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  onMetadata: (listener: (meta: MetadataEvent) => void) => () => void
  height?: number
}

export function GenericStreamView({
  descriptor,
  onFrame,
  onMetadata,
  height,
}: GenericStreamViewProps) {
  switch (descriptor.kind) {
    case 'audio':
      return (
        <>
          <WaveformCanvas
            tap={descriptor.name}
            color={getTapColor(descriptor.name)}
            onFrame={onFrame}
            height={height ?? 80}
            sampleRate={descriptor.sample_rate}
          />
          <SpectrumCanvas
            tap={descriptor.name}
            color={getTapColor(descriptor.name)}
            onFrame={onFrame}
            height={height ?? 80}
            sampleRate={descriptor.sample_rate}
          />
          <SpectrogramCanvas
            tap={descriptor.name}
            color={getTapColor(descriptor.name)}
            onFrame={onFrame}
            height={height ? height * 2 : 160}
            sampleRate={descriptor.sample_rate}
          />
        </>
      )

    case 'metadata': {
      const hasNumeric = descriptor.fields.some(
        (f) => f.field_type === 'f64' || f.field_type === 'u32',
      )
      const hasText = descriptor.fields.some(
        (f) => f.field_type === 'string',
      )

      return (
        <>
          {hasText && (
            <TextStreamView
              streamName={descriptor.name}
              fields={descriptor.fields}
              onMetadata={onMetadata}
            />
          )}
          {hasNumeric && (
            <MetadataStreamView
              streamName={descriptor.name}
              fields={descriptor.fields}
              onMetadata={onMetadata}
              height={height ?? 120}
            />
          )}
          {!hasText && !hasNumeric && (
            <TextStreamView
              streamName={descriptor.name}
              fields={descriptor.fields}
              onMetadata={onMetadata}
            />
          )}
        </>
      )
    }

    case 'text':
      return (
        <TextStreamView
          streamName={descriptor.name}
          fields={[{ name: 'text', field_type: 'string' }]}
          onMetadata={onMetadata}
        />
      )

    default:
      return null
  }
}
