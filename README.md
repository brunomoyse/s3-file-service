# S3 File Service

A Rust web service that turns uploaded product images into PNG, WebP and AVIF variants and stores them on S3. It can also remove image backgrounds through a [rembg](https://github.com/danielgatis/rembg) sidecar.

Every stored image produces six objects, in each of the three formats:

| Variant | Width | Key |
|---|---|---|
| Regular | 600px | `images/{product_slug}.{avif,webp,png}` |
| Thumbnail | 350px | `images/thumbnails/{product_slug}.{avif,webp,png}` |

## API

The server listens on port 8000. It has no authentication, so keep it on a private network. The `POST` routes take `multipart/form-data`.

A `product_slug` is lowercase letters and digits in groups joined by single hyphens, up to 128 characters, for example `california-roll-8`.

### `POST /upload`

Stores the variants of an image as is.

| Field | Type | Required |
|---|---|---|
| `product_slug` | text | yes |
| `image` | file | yes |

Responds with `200` and the text `Images uploaded successfully`.

```sh
curl -F product_slug=my-product -F image=@image.jpg http://127.0.0.1:8000/upload
```

### `POST /images/upload/processed`

Removes the background, trims transparent borders, then stores the variants. It takes the same fields as `/upload` and responds with JSON:

```json
{
  "status": "ok",
  "original_dimensions": { "width": 1200, "height": 800 },
  "post_rembg_dimensions": { "width": 1200, "height": 800 },
  "post_trim_dimensions": { "width": 900, "height": 640 },
  "trim_applied": true,
  "upload_to_s3": true
}
```

### `POST /images/preview/processed`

Runs the same pipeline without storing anything. It only needs the `image` field and responds with the resulting PNG, plus these headers:

- `X-Original-Width`, `X-Original-Height`
- `X-Post-Rembg-Width`, `X-Post-Rembg-Height`
- `X-Post-Trim-Width`, `X-Post-Trim-Height`
- `X-Trim-Applied`

### `DELETE /delete/{product_slug}`

Deletes all six variants and responds with `204`. Variants that don't exist are not an error, so the call is safe to retry.

### Errors

| Status | Meaning |
|---|---|
| `400` | A required field is missing, the slug is invalid, or a processed route received an undecodable image |
| `500` | Decoding, encoding, rembg or S3 failed. `/upload` also answers this for an undecodable image. After a failed delete, some variants may already be gone |
| `503` | A processed route was called but `REMBG_URL` is not set |

## Configuration

Set these as environment variables, or copy `.env.example` to `.env` for local runs.

| Variable | Required | Default | Description |
|---|---|---|---|
| `S3_ACCESS_KEY_ID` | yes | | Access key |
| `S3_SECRET_ACCESS_KEY` | yes | | Secret key |
| `S3_BUCKET_NAME` | yes | | Target bucket |
| `S3_PROVIDER` | no | `aws` | `ovh` selects the OVH defaults, anything else means AWS |
| `S3_REGION` | no | `eu-west-3` (AWS), `gra` (OVH) | Bucket region |
| `S3_ENDPOINT_URL` | no | AWS endpoint, `https://s3.{region}.io.cloud.ovh.net` (OVH) | Custom S3 endpoint |
| `S3_FORCE_PATH_STYLE` | no | `false` (AWS), `true` (OVH) | `true` for path-style bucket addressing |
| `REMBG_URL` | no | | Base URL of the rembg sidecar, required by the processed routes |
| `IMAGE_TRIM_TRANSPARENT` | no | `true` | `false` turns off border trimming |
| `RUST_LOG` | no | `info` | Log filter |

## Development

Requires Rust 1.99 or later, plus `nasm` and a C compiler for the image and TLS libraries.

```sh
cargo run
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The tests mock S3 and rembg, so they need no credentials or network access.

## Docker

```sh
docker build -t s3-file-service .
docker run --env-file .env -p 8000:8000 s3-file-service
```

## Releases

CI runs on every pull request and push, and also verifies that the image builds for `amd64` and `arm64`. Images go to `ghcr.io/brunomoyse/s3-file-service`:

- A push to `main` publishes `:latest`.
- A `v*` tag publishes that version, for example `:v1.0.0`.
