export type ModelEntry = {
  readonly id: string;
  readonly filename: string;
  readonly url: string;
  readonly sizeBytes: number;
  readonly description: string;
  readonly required: boolean;
};

export const MODEL_CATALOG: readonly ModelEntry[] = [
  {
    id: "silero-vad",
    filename: "silero_vad.onnx",
    url: "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx",
    sizeBytes: 2_300_000,
    description: "Silero VAD v5 (voice activity detection)",
    required: true,
  },
  {
    id: "whisper-tiny",
    filename: "ggml-tiny.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
    sizeBytes: 77_700_000,
    description: "Whisper tiny (~75MB, fastest, lower accuracy)",
    required: false,
  },
  {
    id: "whisper-base",
    filename: "ggml-base.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    sizeBytes: 147_500_000,
    description: "Whisper base (~141MB, good balance)",
    required: false,
  },
  {
    id: "whisper-small",
    filename: "ggml-small.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    sizeBytes: 488_000_000,
    description: "Whisper small (~466MB, better accuracy)",
    required: false,
  },
  {
    id: "multilingual-minilm-onnx",
    filename: "multilingual-MiniLM-L12-v2.onnx",
    url: "https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/onnx/model.onnx",
    sizeBytes: 91_000_000,
    description: "Multilingual MiniLM-L12 v2 sentence embeddings (topic detection)",
    required: false,
  },
  {
    id: "multilingual-minilm-tokenizer",
    filename: "multilingual-MiniLM-L12-v2-tokenizer.json",
    url: "https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/tokenizer.json",
    sizeBytes: 1_800_000,
    description: "Tokenizer for multilingual MiniLM-L12 v2",
    required: false,
  },
];

export const DEFAULT_STT_MODEL_ID = "whisper-base";

export function getRequiredModels(): ModelEntry[] {
  return MODEL_CATALOG.filter((m) => m.required);
}

export function getSTTModels(): ModelEntry[] {
  return MODEL_CATALOG.filter((m) => m.id.startsWith("whisper-"));
}

export function findModel(id: string): ModelEntry | undefined {
  return MODEL_CATALOG.find((m) => m.id === id);
}
