// The duck test, through HashiCorp's own Go SDK.
//
// This is the client that matters most, because it is the one written by the
// people who wrote the server. It is also the one that sends the made-up `LIST`
// verb rather than `GET ?list=true`, which is the single easiest way to pass
// every unit test in this repository and still fail `vault kv list`.
package main

import (
	"context"
	"fmt"
	"os"
	"strings"

	vault "github.com/hashicorp/vault/api"
)

var failures []string

func check(name string, fn func() error) {
	if err := fn(); err != nil {
		fmt.Printf("  FAIL  %s: %v\n", name, err)
		failures = append(failures, name)
		return
	}
	fmt.Printf("  ok    %s\n", name)
}

func main() {
	config := vault.DefaultConfig()
	config.Address = os.Getenv("VAULT_ADDR")
	client, err := vault.NewClient(config)
	if err != nil {
		fmt.Println("could not build a client:", err)
		os.Exit(1)
	}
	if token := os.Getenv("VAULT_TOKEN"); token != "" {
		client.SetToken(token)
	}
	ctx := context.Background()
	kv := client.KVv2("secret")

	fmt.Println("go / hashicorp/vault/api")

	check("sys/health", func() error {
		health, err := client.Sys().Health()
		if err != nil {
			return err
		}
		if health.Sealed {
			return fmt.Errorf("a loaded resolver reported sealed")
		}
		if !health.Initialized {
			return fmt.Errorf("reported uninitialised, which makes SDKs give up")
		}
		return nil
	})

	check("auth/token/lookup-self", func() error {
		if _, err := client.Auth().Token().LookupSelf(); err != nil {
			return err
		}
		return nil
	})

	check("sys/mounts", func() error {
		mounts, err := client.Sys().ListMounts()
		if err != nil {
			return err
		}
		if _, ok := mounts["secret/"]; !ok {
			return fmt.Errorf("the configured mount is missing from sys/mounts: %v", mounts)
		}
		return nil
	})

	check("read a secret", func() error {
		secret, err := kv.Get(ctx, "app/db")
		if err != nil {
			return err
		}
		if secret.Data["password"] != "hunter2" {
			return fmt.Errorf("unexpected value: %v", secret.Data)
		}
		if secret.VersionMetadata == nil || secret.VersionMetadata.Version < 1 {
			return fmt.Errorf("no usable version metadata: %+v", secret.VersionMetadata)
		}
		return nil
	})

	check("read a nested secret", func() error {
		secret, err := kv.Get(ctx, "app/sub/deep")
		if err != nil {
			return err
		}
		if secret.Data["k"] != "v" {
			return fmt.Errorf("unexpected value: %v", secret.Data)
		}
		return nil
	})

	// Inside the namespace this token may read. A path outside it answers 403
	// whether or not it exists — a 404 there would be an existence oracle.
	check("missing secret is ErrSecretNotFound", func() error {
		_, err := kv.Get(ctx, "app/nothing-here")
		if err == nil {
			return fmt.Errorf("a missing secret was served")
		}
		if !strings.Contains(err.Error(), "secret not found") &&
			!strings.Contains(err.Error(), "404") {
			return fmt.Errorf("wrong error for a missing secret: %v", err)
		}
		return nil
	})

	// The Go client sends the `LIST` verb, which axum does not route on its
	// own. This is the assertion that catches a server supporting only
	// `GET ?list=true`.
	check("LIST (the verb, not the query parameter)", func() error {
		listed, err := client.Logical().List("secret/metadata/app")
		if err != nil {
			return err
		}
		if listed == nil {
			return fmt.Errorf("LIST returned nothing")
		}
		keys, ok := listed.Data["keys"].([]interface{})
		if !ok || len(keys) == 0 {
			return fmt.Errorf("LIST returned no keys: %v", listed.Data)
		}
		var found, directory bool
		for _, key := range keys {
			if key == "db" {
				found = true
			}
			if key == "sub/" {
				directory = true
			}
		}
		if !found {
			return fmt.Errorf("db missing from the listing: %v", keys)
		}
		if !directory {
			return fmt.Errorf("a nested path did not collapse to a directory entry: %v", keys)
		}
		return nil
	})

	check("read metadata", func() error {
		meta, err := kv.GetVersionsAsList(ctx, "app/db")
		if err != nil {
			return err
		}
		if len(meta) == 0 {
			return fmt.Errorf("no versions reported")
		}
		return nil
	})

	check("every write is refused with 403", func() error {
		attempts := map[string]func() error{
			"Put": func() error {
				_, err := kv.Put(ctx, "app/db", map[string]interface{}{"x": "y"})
				return err
			},
			"Patch": func() error {
				_, err := kv.Patch(ctx, "app/db", map[string]interface{}{"x": "y"})
				return err
			},
			"Delete": func() error { return kv.Delete(ctx, "app/db") },
			"DeleteVersions": func() error {
				return kv.DeleteVersions(ctx, "app/db", []int{1})
			},
			"Undelete": func() error { return kv.Undelete(ctx, "app/db", []int{1}) },
			"Destroy":  func() error { return kv.Destroy(ctx, "app/db", []int{1}) },
		}
		for name, attempt := range attempts {
			err := attempt()
			if err == nil {
				return fmt.Errorf("%s SUCCEEDED — this server must not write", name)
			}
			var responseErr *vault.ResponseError
			if !asResponseError(err, &responseErr) {
				return fmt.Errorf("%s failed without an HTTP status: %v", name, err)
			}
			if responseErr.StatusCode != 403 {
				return fmt.Errorf("%s answered %d, want 403", name, responseErr.StatusCode)
			}
			if len(responseErr.Errors) == 0 || responseErr.Errors[0] != "permission denied" {
				return fmt.Errorf("%s: wrong body %v", name, responseErr.Errors)
			}
		}
		return nil
	})

	if len(failures) > 0 {
		fmt.Printf("\n%d failure(s): %s\n", len(failures), strings.Join(failures, ", "))
		os.Exit(1)
	}
	fmt.Println("\nall go checks passed")
}

func asResponseError(err error, target **vault.ResponseError) bool {
	for err != nil {
		if responseErr, ok := err.(*vault.ResponseError); ok {
			*target = responseErr
			return true
		}
		unwrapper, ok := err.(interface{ Unwrap() error })
		if !ok {
			return false
		}
		err = unwrapper.Unwrap()
	}
	return false
}
