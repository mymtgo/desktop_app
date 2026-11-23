import licencesJson from '@/content/licences.json?raw'
import { parseLicences } from '@/lib/licences'

// Read as text and parsed once: the file is large, and type-checking it as a
// JSON module would slow `vue-tsc` for no gain.
export const licenceList = parseLicences(licencesJson)
