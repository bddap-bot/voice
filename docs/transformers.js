export async function transformers() {
  const library = await import('https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.7.3');
  library.env.allowLocalModels = false;
  return library;
}
