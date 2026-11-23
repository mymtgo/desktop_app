/** The app's log folder inside its data folder, written with the data folder's own separator. */
export function logFolder(dataDir: string): string {
  const separator = dataDir.includes('\\') ? '\\' : '/'
  return `${dataDir.replace(/[\\/]+$/, '')}${separator}logs`
}
