import { existsSync } from 'node:fs';
const edge = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
export const browserPath = process.env.UVMS_BROWSER_PATH || process.env.UVMS_BROWSER || (existsSync(edge) ? edge : undefined);
