# Get Page (Markdown Format)

**GET /page/:id**

**Request Headers**

```
Authorization: Bearer <NOTION_API_KEY>
Content-Type: text/markdown
```

**Query Parameters**

- `frontmatter` (optional, boolean, default: false): If true, includes frontmatter metadata in the markdown response.

**Response**

String

**Sample Response**

```markdown
---
title: Sample Page
author: John Doe
created: 2024-01-01
---

# Sample Page
This is a sample page content in markdown format.
```

**Status Codes**

- `200 OK`: The request was successful, and the page content is returned in markdown format.
- `400 Bad Request`: The request was malformed or contained invalid parameters.
- `401 Unauthorized`: The provided API key is invalid or missing.
- `404 Not Found`: The specified page ID does not exist.
- `500 Internal Server Error`: An error occurred on the server while processing the request.
