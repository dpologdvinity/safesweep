#!/usr/bin/env bash
# Builds a signed apt repository (one suite, `stable`) from the .deb files in DEBS into OUT,
# for GitHub Pages. The signing key must already be in the gpg keyring.
# Usage: apt/build.sh DEBS OUT KEY_FINGERPRINT
set -euo pipefail
debs=$1 out=$2 key=$3
url=https://dpologdvinity.github.io/safesweep
mkdir -p "$out/pool/main" "$out/dists/stable/main/binary-amd64" "$out/dists/stable/main/binary-arm64"
cp "$debs"/*.deb "$out/pool/main/"
cd "$out"
for arch in amd64 arm64; do
  apt-ftparchive --arch "$arch" packages pool > "dists/stable/main/binary-$arch/Packages"
  gzip -9kf "dists/stable/main/binary-$arch/Packages"
done
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=safesweep \
  -o APT::FTPArchive::Release::Label=safesweep \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
  -o APT::FTPArchive::Release::Components=main \
  release dists/stable > Release.tmp # written outside dists/ so it does not index itself
mv Release.tmp dists/stable/Release
gpg --batch --yes --local-user "$key" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$key" --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
gpg --batch --armor --export "$key" > key.asc
cat > index.html <<HTML
<!doctype html><meta charset="utf-8"><title>safesweep apt repository</title>
<h1>safesweep apt repository</h1>
<pre>
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL $url/key.asc | sudo tee /etc/apt/keyrings/safesweep.asc >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/safesweep.asc] $url stable main" | sudo tee /etc/apt/sources.list.d/safesweep.list
sudo apt update &amp;&amp; sudo apt install safesweep
</pre>
<p>Key fingerprint: <code>$key</code></p>
<p><a href="https://github.com/dpologdvinity/safesweep">github.com/dpologdvinity/safesweep</a></p>
HTML
touch .nojekyll # serve dists/ and pool/ as-is
