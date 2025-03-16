# Image Processing and S3 Uploader

## Overview
This project is a Rust-based web service that processes uploaded images, generates PNG, WebP, and AVIF versions in both regular and thumbnail sizes, and uploads them to an Amazon S3 bucket. The service is built using **Actix Web** and utilizes **webp**, **ravif**, and **image** crates for image processing.

## Features
- Accepts image uploads via an HTTP endpoint.
- Resizes images to **600px width** (regular) and **350px width** (thumbnail).
- Converts images to **PNG, WebP, and AVIF** formats.
- Uploads processed images to an **AWS S3 bucket**.
- Optimized for performance with parallel processing.

## Tech Stack
- **Rust** (safe and performant backend)
- **Actix Web** (handling HTTP requests)
- **Tokio** (asynchronous execution)
- **Image Processing:**
  - `image` crate (resizing, PNG encoding)
  - `webp` crate (WebP encoding)
  - `ravif` crate (AVIF encoding)
- **AWS SDK for Rust** (uploading to S3)
- **dotenv** (loading environment variables)

## Installation
1. Clone the repository:
   ```sh
   git clone https://github.com/yourusername/your-repo.git
   cd your-repo
   ```
2. Install Rust (if not installed):
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
3. Install dependencies:
   ```sh
   cargo build
   ```

## Configuration
Set up a `.env` file in the project root with the following environment variables:
```env
AWS_ACCESS_KEY_ID=your_access_key_id
AWS_SECRET_ACCESS_KEY=your_secret_access_key
AWS_S3_BUCKET_NAME=your_s3_bucket
```

## Running the Service
Start the server using:
```sh
cargo run
```
The service will be available at `http://127.0.0.1:8000`.

## API Usage
### Upload Image
- **Endpoint:** `POST /upload`
- **Request Type:** `multipart/form-data`
- **Required Fields:**
  - `product_slug`: (string) Slug identifier for the product
  - `image`: (file) Image file to be processed

#### Example cURL Request:
```sh
curl -X POST "http://127.0.0.1:8000/upload" \
     -F "product_slug=my-product" \
     -F "image=@/path/to/image.jpg"
```

### Expected Behavior
- The image is resized to **600px (regular) and 350px (thumbnail)**.
- The image is converted to **PNG, WebP, and AVIF** formats.
- All versions are uploaded to an **S3 bucket** under:
  - `images/{product_slug}.{format}`
  - `images/thumbnails/{product_slug}.{format}`
- Returns a success message upon completion.

## Performance Optimizations
- Uses **web::block** for CPU-bound image processing.
- Processes all image formats in a **single batch operation**.
- Uploads to S3 **in parallel** using `tokio::spawn`.

## Start with Docker
```bash
$ docker build -t s3-file-service .
$ docker run --name s3-file-service --env-file .env -p 8000:8000 s3-file-service
```
