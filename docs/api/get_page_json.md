# Get Page (JSON Format)

**GET /page/:id**

**Request Headers**

```
Authorization: Bearer <NOTION_API_KEY>
Content-Type: application/json
```

**Response**

```rust
struct GetPageResponse {
    // The unique identifier of the page
    id: String,
    // the properties of the page
    properties: HashMap<String, Union<String, Number, Boolean, Array<String>, DateTime<Utc>>>>,
    // The content of the page in markdown format
    content: String,
}
```

**Sample Response**

```json
{
    "id": "page_id_123",
    "properties": {
        "Title": "Sample Page",
        "Author": "John Doe",
        "Created": "2024-01-01"
    },
    "content": "# Sample Page\nThis is a sample page content in markdown format."
}
```

**Status Codes**

- `200 OK`: The request was successful, and the page content is returned in markdown format.
- `400 Bad Request`: The request was malformed or contained invalid parameters.
- `401 Unauthorized`: The provided API key is invalid or missing.
- `404 Not Found`: The specified page ID does not exist.
- `500 Internal Server Error`: An error occurred on the server while processing the request.
