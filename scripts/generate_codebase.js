#!/usr/bin/env node
/*
 * generate_codebase.js
 * --------------------
 * Walks the repository and writes all file contents into a single file
 * called `codebase.txt`. Each file is preceded by a comment containing
 * its relative path. Directories such as `.git`, `node_modules`, `target`,
 * and others are skipped to keep the output compact.
 *
 * Run with:
 *   node scripts/generate_codebase.js
 */

const fs = require("fs");
const path = require("path");

const OUTPUT_FILE = "codebase.txt";

// Directories and files to skip while traversing
const EXCLUDE = new Set([
	".git",
	"node_modules",
	"target",
	".anchor",
	".DS_Store",
	".gitignore",
	OUTPUT_FILE,
]);

/**
 * Recursively walk a directory and collect paths to files, skipping any paths
 * that match our exclusion rules.
 * @param {string} dir absolute directory to walk
 * @param {string[]} acc accumulator array of relative file paths
 * @param {string} root original root directory to compute relative paths
 */
function walk(dir, acc, root) {
	const entries = fs.readdirSync(dir, { withFileTypes: true });
	for (const entry of entries) {
		// Skip hidden directories or files that are in our EXCLUDE set
		if (EXCLUDE.has(entry.name)) continue;

		const fullPath = path.join(dir, entry.name);
		const relPath = path.relative(root, fullPath);

		if (entry.isDirectory()) {
			walk(fullPath, acc, root);
		} else if (entry.isFile()) {
			acc.push(relPath);
		}
	}
}

/**
 * Main execution
 */
(function main() {
	const repoRoot = path.resolve(__dirname, "..");
	const files = [];
	walk(repoRoot, files, repoRoot);

	const out = fs.createWriteStream(path.join(repoRoot, OUTPUT_FILE));

	for (const file of files) {
		const content = fs.readFileSync(path.join(repoRoot, file), "utf8");

		// Choose comment prefix depending on file extension (default '//')
		const ext = path.extname(file);
		const commentPrefix =
			ext === ".rs" || ext === ".js" || ext === ".ts" || ext === ".tsx" || ext === ".jsx"
				? "//"
				: "#";

		out.write(`${commentPrefix} ${file}\n`);
		out.write(content);
		if (!content.endsWith("\n")) out.write("\n");
		out.write("\n"); // extra newline between files
	}

	out.end(() => console.log(`✅  Generated ${OUTPUT_FILE} with ${files.length} files`));
})();
