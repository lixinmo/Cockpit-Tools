// Compatibility entry point: this command dispatches GitHub Actions only.
console.log('build:app:local now uses GitHub Actions. No local compilation is performed.');
require('./build-cloud.cjs');
