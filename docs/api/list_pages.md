# List Pages

**GET /database/:id**

**Request Headers**

```
Authorization: Bearer <NOTION_API_KEY>
Content-Type: application/json
```

**Response**

```rust
struct ListPagesResponse {
    // The total number of pages in the database
    total: u32,
    // The offset for pagination
    offset: u32,
    // The maximum number of pages returned
    limit: u32,
    // The list of page IDs
    pages: Vec<String>,
}
```

**Sample Response**

```json
{
    "total": 100,
    "offset": 3,
    "limit": 20,
    "pages": [
        "page1_id",
        "page2_id",
        "page3_id"
        ...
    ]
}
```

**Status Codes**

- `200 OK`: The request was successful, and the page content is returned in markdown format.
- `400 Bad Request`: The request was malformed or contained invalid parameters.
- `401 Unauthorized`: The provided API key is invalid or missing.
- `404 Not Found`: The specified database ID does not exist.
- `500 Internal Server Error`: An error occurred on the server while processing the request.
