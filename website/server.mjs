import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const portArgument = process.argv.indexOf("--port");
const port = Number(portArgument >= 0 ? process.argv[portArgument + 1] : process.env.PORT ?? 4173);

const contentTypes = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
};

function resolveRequest(pathname) {
  const decoded = decodeURIComponent(pathname);
  const requested = decoded.endsWith("/") ? `${decoded}index.html` : decoded;
  const candidate = resolve(root, `.${requested}`);
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) return null;
  return candidate;
}

createServer((request, response) => {
  const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
  let file = resolveRequest(pathname);
  try {
    if (!file || !statSync(file).isFile()) throw new Error("not found");
  } catch {
    file = resolve(root, "404.html");
    response.statusCode = 404;
  }

  response.setHeader("Content-Type", contentTypes[extname(file)] ?? "application/octet-stream");
  response.setHeader("Cache-Control", "no-store");
  createReadStream(file).pipe(response);
}).listen(port, "127.0.0.1", () => {
  console.log(`SophoNote website: http://127.0.0.1:${port}`);
});
