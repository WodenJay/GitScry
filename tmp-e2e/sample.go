package main

type Client struct {
	Name string
}

func (c *Client) Request(path string) error {
	if path == "" {
		return nil
	}
	return nil
}

func main() {
	c := &Client{}
	c.Request("x")
}
