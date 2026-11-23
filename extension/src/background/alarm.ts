// The repeating ping alarm. It survives the worker stopping, so a load only creates it when the
// browser does not already have it; creating it again would restart its countdown.
export interface AlarmApi {
  get(name: string): Promise<unknown>
  create(name: string, info: { periodInMinutes: number }): void | Promise<void>
}

export async function ensureAlarm(api: AlarmApi, name: string, periodInMinutes: number): Promise<void> {
  try {
    if (await api.get(name)) return
    await api.create(name, { periodInMinutes })
  } catch (error) {
    console.warn('mymtgo-endstep: could not set the ping alarm', error)
  }
}
