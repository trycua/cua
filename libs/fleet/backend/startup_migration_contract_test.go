package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

var databaseExecutionMethods = map[string]struct{}{
	"Exec": {}, "ExecContext": {}, "Query": {}, "QueryContext": {},
	"QueryRow": {}, "QueryRowContext": {}, "Prepare": {}, "PrepareContext": {},
}

func readSource(t *testing.T, name string) string {
	t.Helper()

	contents, err := os.ReadFile(filepath.Clean(name))
	if err != nil {
		t.Fatal(err)
	}
	return string(contents)
}

func isGeneratedGoSource(file *ast.File) bool {
	return ast.IsGenerated(file)
}

type migrationImportSet struct {
	statequeryAliases map[string]bool
	databaseAliases   map[string]bool
	sqlAliases        map[string]bool
	pgxAliases        map[string]bool
	pgxPoolAliases    map[string]bool
	statequeryDot     bool
	databaseDot       bool
}

func migrationImports(file *ast.File) migrationImportSet {
	imports := migrationImportSet{
		statequeryAliases: map[string]bool{},
		databaseAliases:   map[string]bool{},
		sqlAliases:        map[string]bool{},
		pgxAliases:        map[string]bool{},
		pgxPoolAliases:    map[string]bool{},
	}
	for _, importSpec := range file.Imports {
		path, err := strconv.Unquote(importSpec.Path.Value)
		if err != nil {
			continue
		}
		alias := filepath.Base(path)
		if importSpec.Name != nil {
			alias = importSpec.Name.Name
		}
		switch path {
		case "cyclops-cs-backend/statequery":
			if alias == "." {
				imports.statequeryDot = true
			} else {
				imports.statequeryAliases[alias] = true
			}
		case "cyclops-cs-backend/database":
			if alias == "." {
				imports.databaseDot = true
			} else {
				imports.databaseAliases[alias] = true
			}
		case "database/sql":
			imports.sqlAliases[alias] = true
		case "github.com/jackc/pgx/v5":
			imports.pgxAliases[alias] = true
		case "github.com/jackc/pgx/v5/pgxpool":
			imports.pgxPoolAliases[alias] = true
		}
	}
	return imports
}

func migrationAuthorityViolation(file *ast.File) string {
	imports := migrationImports(file)
	packageConstants := collectPackageStringConstants(file.Decls)
	var forbidden string

	ast.Inspect(file, func(node ast.Node) bool {
		if forbidden != "" {
			return false
		}
		switch value := node.(type) {
		case *ast.CommentGroup:
			if strings.Contains(value.Text(), "go:embed") && strings.Contains(value.Text(), "migrations/") {
				forbidden = "embedded legacy migration SQL"
			}
		case *ast.BasicLit:
			if value.Kind == token.STRING {
				literal, err := strconv.Unquote(value.Value)
				if err == nil && isLegacyMigrationPath(literal) {
					forbidden = "legacy migration SQL"
				}
			}
		case *ast.FuncDecl:
			constants := collectLocalStringConstants(value.Body.List, packageConstants)
			receivers := databaseReceiverContracts(value.Type, imports)
			if violation := scanFunctionCalls(value.Body, constants, imports, receivers); violation != "" {
				forbidden = violation
			}
			return false
		case *ast.FuncLit:
			constants := collectLocalStringConstants(value.Body.List, packageConstants)
			receivers := databaseReceiverContracts(value.Type, imports)
			if violation := scanFunctionCalls(value.Body, constants, imports, receivers); violation != "" {
				forbidden = violation
			}
			return false
		}
		return true
	})
	if forbidden != "" {
		return forbidden
	}

	var entrypoint string
	ast.Inspect(file, func(node ast.Node) bool {
		if entrypoint != "" {
			return false
		}
		call, ok := node.(*ast.CallExpr)
		if ok && isMigrationEntrypointCall(call, imports) {
			entrypoint = "migration entrypoint"
		}
		return true
	})
	return entrypoint
}

func scanFunctionCalls(body *ast.BlockStmt, constants map[string]string, imports migrationImportSet, receivers map[string]databaseExecutionContract) string {
	var forbidden string
	ast.Inspect(body, func(node ast.Node) bool {
		if forbidden != "" {
			return false
		}
		if function, ok := node.(*ast.FuncLit); ok && function.Body != body {
			return false
		}
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		if violation := scanMigrationCall(call, constants, imports, receivers); violation != "" {
			forbidden = violation
		}
		return true
	})
	return forbidden
}

func scanMigrationCall(call *ast.CallExpr, constants map[string]string, imports migrationImportSet, receivers map[string]databaseExecutionContract) string {
	if identifier, ok := call.Fun.(*ast.Ident); ok {
		switch {
		case identifier.Name == "AutoMigrate":
			return "AutoMigrate"
		case imports.databaseDot && identifier.Name == "Run":
			return "database.Run"
		case imports.statequeryDot && (identifier.Name == "Migrate" || identifier.Name == "ReconcileFixedRoles"):
			return "statequery migration entrypoint"
		}
	}
	if isMigrationEntrypointCall(call, imports) {
		return "migration entrypoint"
	}
	if !isDatabaseExecutionCall(call) {
		return ""
	}
	for _, argument := range sqlArguments(call, receivers) {
		query, ok := foldString(argument, constants)
		if !ok {
			continue
		}
		if isLegacyMigrationPath(query) {
			return "legacy migration SQL"
		}
		if isSchemaOrPrivilegeSQL(query) {
			return "direct schema or privilege SQL"
		}
	}
	return ""
}

func isMigrationEntrypointCall(call *ast.CallExpr, imports migrationImportSet) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	identifier, ok := selector.X.(*ast.Ident)
	if !ok {
		return false
	}
	return (imports.databaseAliases[identifier.Name] && selector.Sel.Name == "Run") ||
		(imports.statequeryAliases[identifier.Name] && (selector.Sel.Name == "Migrate" || selector.Sel.Name == "ReconcileFixedRoles")) ||
		(imports.databaseDot && selector.Sel.Name == "Run") ||
		(imports.statequeryDot && (selector.Sel.Name == "Migrate" || selector.Sel.Name == "ReconcileFixedRoles"))
}

type databaseExecutionContract uint8

const (
	unknownDatabaseExecution databaseExecutionContract = iota
	databaseSQLExecution
	pgxExecution
)

func databaseReceiverContracts(function *ast.FuncType, imports migrationImportSet) map[string]databaseExecutionContract {
	receivers := map[string]databaseExecutionContract{}
	if function == nil || function.Params == nil {
		return receivers
	}
	for _, field := range function.Params.List {
		contract := databaseExecutionContractForType(field.Type, imports)
		if contract == unknownDatabaseExecution {
			continue
		}
		for _, name := range field.Names {
			receivers[name.Name] = contract
		}
	}
	return receivers
}

func databaseExecutionContractForType(expression ast.Expr, imports migrationImportSet) databaseExecutionContract {
	for {
		pointer, ok := expression.(*ast.StarExpr)
		if !ok {
			break
		}
		expression = pointer.X
	}
	selector, ok := expression.(*ast.SelectorExpr)
	if !ok {
		return unknownDatabaseExecution
	}
	packageName, ok := selector.X.(*ast.Ident)
	if !ok {
		return unknownDatabaseExecution
	}
	switch {
	case imports.sqlAliases[packageName.Name] && (selector.Sel.Name == "DB" || selector.Sel.Name == "Conn" || selector.Sel.Name == "Tx"):
		return databaseSQLExecution
	case imports.pgxAliases[packageName.Name] && (selector.Sel.Name == "Conn" || selector.Sel.Name == "Tx"):
		return pgxExecution
	case imports.pgxPoolAliases[packageName.Name] && selector.Sel.Name == "Pool":
		return pgxExecution
	default:
		return unknownDatabaseExecution
	}
}

func isDatabaseExecutionCall(call *ast.CallExpr) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	_, ok = databaseExecutionMethods[selector.Sel.Name]
	return ok
}

func sqlArguments(call *ast.CallExpr, receivers map[string]databaseExecutionContract) []ast.Expr {
	selector := call.Fun.(*ast.SelectorExpr)
	var contract databaseExecutionContract
	if receiver, ok := selector.X.(*ast.Ident); ok {
		contract = receivers[receiver.Name]
	}
	arguments := make([]ast.Expr, 0, 3)
	for _, index := range sqlArgumentIndexes(selector.Sel.Name, contract) {
		if index < len(call.Args) {
			arguments = append(arguments, call.Args[index])
		}
	}
	return arguments
}

func sqlArgumentIndexes(method string, contract databaseExecutionContract) []int {
	switch contract {
	case pgxExecution:
		if method == "Prepare" {
			return []int{2}
		}
		return []int{1}
	case databaseSQLExecution:
		if strings.HasSuffix(method, "Context") {
			return []int{1}
		}
		return []int{0}
	default:
		if strings.HasSuffix(method, "Context") {
			return []int{1}
		}
		if method == "Prepare" {
			return []int{0, 1, 2}
		}
		return []int{0, 1}
	}
}

func collectPackageStringConstants(declarations []ast.Decl) map[string]string {
	constants := map[string]string{}
	for _, declaration := range declarations {
		gen, ok := declaration.(*ast.GenDecl)
		if !ok || gen.Tok != token.CONST {
			continue
		}
		collectStringValueSpecs(constants, gen.Specs)
	}
	return constants
}

func collectLocalStringConstants(statements []ast.Stmt, base map[string]string) map[string]string {
	constants := mergeStringConstants(base, nil)
	for _, statement := range statements {
		declaration, ok := statement.(*ast.DeclStmt)
		if !ok {
			continue
		}
		gen, ok := declaration.Decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.CONST {
			continue
		}
		collectStringValueSpecs(constants, gen.Specs)
	}
	return constants
}

func collectStringValueSpecs(constants map[string]string, specifications []ast.Spec) {
	for _, specification := range specifications {
		valueSpec, ok := specification.(*ast.ValueSpec)
		if !ok {
			continue
		}
		for index, name := range valueSpec.Names {
			if index >= len(valueSpec.Values) {
				continue
			}
			if value, ok := foldString(valueSpec.Values[index], constants); ok {
				constants[name.Name] = value
			}
		}
	}
}

func mergeStringConstants(base, additional map[string]string) map[string]string {
	merged := make(map[string]string, len(base)+len(additional))
	for name, value := range base {
		merged[name] = value
	}
	for name, value := range additional {
		merged[name] = value
	}
	return merged
}

func foldString(expression ast.Expr, constants map[string]string) (string, bool) {
	switch value := expression.(type) {
	case *ast.BasicLit:
		if value.Kind != token.STRING {
			return "", false
		}
		unquoted, err := strconv.Unquote(value.Value)
		return unquoted, err == nil
	case *ast.ParenExpr:
		return foldString(value.X, constants)
	case *ast.BinaryExpr:
		if value.Op != token.ADD {
			return "", false
		}
		left, leftOK := foldString(value.X, constants)
		right, rightOK := foldString(value.Y, constants)
		return left + right, leftOK && rightOK
	case *ast.Ident:
		constant, ok := constants[value.Name]
		return constant, ok
	default:
		return "", false
	}
}

func isLegacyMigrationPath(value string) bool {
	return strings.Contains(filepath.ToSlash(value), "database/migrations/")
}

func isSchemaOrPrivilegeSQL(query string) bool {
	for _, statement := range splitSQLStatements(query) {
		statement = stripLeadingSQLComments(statement)
		upper := strings.ToUpper(statement)
		switch {
		case startsSQLKeyword(upper, "CREATE"), startsSQLKeyword(upper, "ALTER"), startsSQLKeyword(upper, "DROP"),
			startsSQLKeyword(upper, "GRANT"), startsSQLKeyword(upper, "REVOKE"),
			startsSQLPhrase(upper, "COMMENT ON"), startsSQLPhrase(upper, "SECURITY LABEL"),
			startsSQLPhrase(upper, "REASSIGN OWNED"), startsSQLPhrase(upper, "DROP OWNED"):
			return true
		}
	}
	return false
}

func startsSQLKeyword(statement, keyword string) bool {
	if !strings.HasPrefix(statement, keyword) {
		return false
	}
	return len(statement) == len(keyword) || !isSQLIdentifierByte(statement[len(keyword)])
}

func startsSQLPhrase(statement, phrase string) bool {
	if !strings.HasPrefix(statement, phrase) {
		return false
	}
	return len(statement) == len(phrase) || !isSQLIdentifierByte(statement[len(phrase)])
}

func isSQLIdentifierByte(value byte) bool {
	return value == '_' || value >= 'A' && value <= 'Z' || value >= '0' && value <= '9'
}

func stripLeadingSQLComments(statement string) string {
	for {
		statement = strings.TrimLeft(statement, " \t\r\n\f")
		switch {
		case strings.HasPrefix(statement, "--"):
			if newline := strings.IndexByte(statement, '\n'); newline >= 0 {
				statement = statement[newline+1:]
				continue
			}
			return ""
		case strings.HasPrefix(statement, "/*"):
			end := strings.Index(statement[2:], "*/")
			if end < 0 {
				return ""
			}
			statement = statement[end+4:]
			continue
		default:
			return statement
		}
	}
}

func splitSQLStatements(query string) []string {
	var statements []string
	start := 0
	inSingleQuote := false
	inDoubleQuote := false
	inLineComment := false
	inBlockComment := false
	for index := 0; index < len(query); index++ {
		if inLineComment {
			if query[index] == '\n' {
				inLineComment = false
			}
			continue
		}
		if inBlockComment {
			if query[index] == '*' && index+1 < len(query) && query[index+1] == '/' {
				inBlockComment = false
				index++
			}
			continue
		}
		if inSingleQuote {
			if query[index] == '\'' {
				if index+1 < len(query) && query[index+1] == '\'' {
					index++
				} else {
					inSingleQuote = false
				}
			}
			continue
		}
		if inDoubleQuote {
			if query[index] == '"' {
				if index+1 < len(query) && query[index+1] == '"' {
					index++
				} else {
					inDoubleQuote = false
				}
			}
			continue
		}
		switch query[index] {
		case '-':
			if index+1 < len(query) && query[index+1] == '-' {
				inLineComment = true
				index++
			}
		case '/':
			if index+1 < len(query) && query[index+1] == '*' {
				inBlockComment = true
				index++
			}
		case '\'':
			inSingleQuote = true
		case '"':
			inDoubleQuote = true
		case ';':
			statements = append(statements, query[start:index])
			start = index + 1
		}
	}
	return append(statements, query[start:])
}

func scanActiveBackend(root string) error {
	return filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		relative = filepath.ToSlash(relative)
		if entry.IsDir() {
			if relative == "vendor" || relative == "database/migrations" || relative == "cmd/db-migrate" {
				return filepath.SkipDir
			}
			return nil
		}
		if strings.HasSuffix(relative, ".sql") {
			return fmt.Errorf("%s is runtime-loaded SQL outside database/migrations", relative)
		}
		if !strings.HasSuffix(relative, ".go") || strings.HasSuffix(relative, "_test.go") || relative == "database/migrator.go" {
			return nil
		}
		file, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.ParseComments)
		if err != nil {
			return err
		}
		if isGeneratedGoSource(file) {
			return nil
		}
		if forbidden := migrationAuthorityViolation(file); forbidden != "" {
			return fmt.Errorf("%s retains migration authority via %s", relative, forbidden)
		}
		return nil
	})
}

func parseFixture(t *testing.T, source string) *ast.File {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), "fixture.go", source, parser.ParseComments)
	if err != nil {
		t.Fatal(err)
	}
	return file
}

func TestGeneratedSourceIsExcludedFromMigrationAuthorityScan(t *testing.T) {
	generated := parseFixture(t, `// Code generated by contract fixture. DO NOT EDIT.
package test
func f() { db.Exec(ctx, "create table x (id int)") }`)
	if !isGeneratedGoSource(generated) {
		t.Fatal("generated source was not excluded")
	}

	spoofed := parseFixture(t, `package test
// Code generated by contract fixture. DO NOT EDIT.
func f() { db.Exec(ctx, "create table x (id int)") }`)
	if isGeneratedGoSource(spoofed) {
		t.Fatal("comment after package must not exempt source")
	}
}

func TestGitHubTrustNewDoesNotMigrate(t *testing.T) {
	source := readSource(t, "githubtrust/store.go")
	for _, forbidden := range []string{"AutoMigrate", "gorm.Open", "gorm.io/driver/postgres", "gorm.io/gorm"} {
		if strings.Contains(source, forbidden) {
			t.Fatalf("store startup still contains %q", forbidden)
		}
	}
}

func TestMigrationAuthorityScannerCatchesForbiddenEntries(t *testing.T) {
	for name, source := range map[string]string{
		"database alias":             `package test; import db "cyclops-cs-backend/database"; func f() { db.Run() }`,
		"database dot import":        `package test; import . "cyclops-cs-backend/database"; func f() { Run() }`,
		"statequery alias":           `package test; import sq "cyclops-cs-backend/statequery"; func f() { sq.Migrate() }`,
		"statequery dot import":      `package test; import . "cyclops-cs-backend/statequery"; func f() { Migrate() }`,
		"automigrate":                `package test; func f() { AutoMigrate() }`,
		"comment-prefixed DDL":       "package test\nfunc f() { db.Exec(ctx, " + strconv.Quote("-- audit\n/* allow */ create table x (id int)") + `) }`,
		"create domain":              `package test; func f() { db.Exec(ctx, "create domain email as text") }`,
		"create event trigger":       `package test; func f() { db.Exec(ctx, "create event trigger audit on ddl_command_end execute function f()") }`,
		"alter default privileges":   `package test; func f() { db.Exec(ctx, "alter default privileges grant select on tables to app") }`,
		"concatenated create table":  `package test; const prefix = "create"; func f() { const query = (prefix + " table x (id int)"); db.Exec(ctx, query) }`,
		"legacy migration":           `package test; var migration = "database/migrations/000001_initial_schema.sql"`,
		"legacy migration execution": `package test; func f() { db.Exec(ctx, "database/migrations/000001_initial_schema.sql") }`,
		"comment on":                 `package test; func f() { db.Exec(ctx, "comment on table x is 'x'") }`,
		"security label":             `package test; func f() { db.Exec(ctx, "security label on table x is 'x'") }`,
		"reassign owned":             `package test; func f() { db.Exec(ctx, "reassign owned by old_role to new_role") }`,
		"grant":                      `package test; func f() { db.Exec(ctx, "grant select on x to app") }`,
		"revoke":                     `package test; func f() { db.Exec(ctx, "revoke select on x from app") }`,
		"pgx pool exec": `package test
import (
  "context"
  "github.com/jackc/pgx/v5/pgxpool"
)
func f(pool *pgxpool.Pool, requestContext context.Context) { pool.Exec(requestContext, "create table x (id int)") }`,
		"pgx conn query": `package test
import (
  "context"
  "github.com/jackc/pgx/v5"
)
func f(conn *pgx.Conn, operationContext context.Context) { conn.Query(operationContext, "create table x (id int)") }`,
		"pgx tx query row": `package test
import (
  "context"
  "github.com/jackc/pgx/v5"
)
func f(tx pgx.Tx, callerContext context.Context) { tx.QueryRow(callerContext, "create table x (id int)") }`,
		"pgx prepare": `package test
import (
  "context"
  "github.com/jackc/pgx/v5/pgxpool"
)
func f(pool *pgxpool.Pool, requestContext context.Context) { pool.Prepare(requestContext, "named", "create table x (id int)") }`,
		"pgx tx prepare": `package test
import (
  "context"
  "github.com/jackc/pgx/v5"
)
func f(tx pgx.Tx, requestContext context.Context) { tx.Prepare(requestContext, "named", "create table x (id int)") }`,
		"database sql exec": `package test
import "database/sql"
func f(db *sql.DB) { db.Exec("create table x (id int)", 1) }`,
		"database sql query": `package test
import "database/sql"
func f(db *sql.DB) { db.Query("create table x (id int)", 1) }`,
		"database sql query row": `package test
import "database/sql"
func f(db *sql.DB) { db.QueryRow("create table x (id int)", 1) }`,
		"database sql prepare": `package test
import "database/sql"
func f(db *sql.DB) { db.Prepare("create table x (id int)") }`,
		"database sql exec context": `package test
import (
  "context"
  "database/sql"
)
func f(db *sql.DB, requestContext context.Context) { db.ExecContext(requestContext, "create table x (id int)") }`,
		"database sql query context": `package test
import (
  "context"
  "database/sql"
)
func f(db *sql.DB, requestContext context.Context) { db.QueryContext(requestContext, "create table x (id int)") }`,
		"database sql query row context": `package test
import (
  "context"
  "database/sql"
)
func f(db *sql.DB, requestContext context.Context) { db.QueryRowContext(requestContext, "create table x (id int)") }`,
		"database sql prepare context": `package test
import (
  "context"
  "database/sql"
)
func f(db *sql.DB, requestContext context.Context) { db.PrepareContext(requestContext, "create table x (id int)") }`,
	} {
		t.Run(name, func(t *testing.T) {
			if migrationAuthorityViolation(parseFixture(t, source)) == "" {
				t.Fatal("migration-authority scanner missed forbidden source")
			}
		})
	}
}

func TestMigrationAuthorityScannerAllowsCRUD(t *testing.T) {
	for _, source := range []string{
		`package test
import (
  "context"
  "github.com/jackc/pgx/v5/pgxpool"
)
func f(pool *pgxpool.Pool, requestContext context.Context) { pool.Exec(requestContext, "insert into widgets (id) values ($1)") }`,
		`package test
import "database/sql"
func f(db *sql.DB) { db.Query("select * from widgets where id = $1", 1) }`,
		`package test
import (
  "context"
  "database/sql"
)
func f(db *sql.DB, requestContext context.Context) { db.QueryRowContext(requestContext, "update widgets set name = $1 where id = $2 returning id") }`,
		`package test
import "database/sql"
func f(db *sql.DB) { db.Prepare("delete from widgets where id = $1") }`,
	} {
		if violation := migrationAuthorityViolation(parseFixture(t, source)); violation != "" {
			t.Fatalf("normal CRUD query reported as %s", violation)
		}
	}
}

func TestActiveBackendScannerRejectsRuntimeSQLAndHonorsCanonicalGeneratedFiles(t *testing.T) {
	root := t.TempDir()
	writeFixture := func(name, source string) {
		t.Helper()
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(source), 0o600); err != nil {
			t.Fatal(err)
		}
	}

	writeFixture("runtime.sql", "select 1")
	if err := scanActiveBackend(root); err == nil || !strings.Contains(err.Error(), "runtime.sql") {
		t.Fatalf("runtime SQL was not rejected: %v", err)
	}
	if err := os.Remove(filepath.Join(root, "runtime.sql")); err != nil {
		t.Fatal(err)
	}
	writeFixture("generated.go", "// Code generated by fixture. DO NOT EDIT.\npackage fixture\nfunc f() { db.Exec(ctx, \"create table x (id int)\") }\n")
	if err := scanActiveBackend(root); err != nil {
		t.Fatalf("canonical generated file was not excluded: %v", err)
	}
	writeFixture("spoofed.go", "package fixture\n// Code generated by fixture. DO NOT EDIT.\nfunc f() { db.Exec(ctx, \"create table x (id int)\") }\n")
	if err := scanActiveBackend(root); err == nil || !strings.Contains(err.Error(), "spoofed.go") {
		t.Fatalf("spoofed generated comment was accepted: %v", err)
	}
}

func TestActiveBackendSourcesDoNotOwnMigrations(t *testing.T) {
	if err := scanActiveBackend("."); err != nil {
		t.Fatal(err)
	}
}

func TestMainRequiresMigrationVersionWithoutRunningMigrations(t *testing.T) {
	source := readSource(t, "main.go")
	if !strings.Contains(source, "database.RequireVersion(ctx, cfg.Database.URL, 1)") {
		t.Fatal("backend must require migration version 1")
	}
}

func TestBackendReadinessStartsFailClosedAndRequiresSchema(t *testing.T) {
	source := readSource(t, "main.go")
	if !strings.Contains(source, "h.Readiness = handlers.NewReadiness()") {
		t.Fatal("backend readiness must start fail-closed before the database check")
	}
	if !strings.Contains(source, "h.Readiness.MarkReady()") {
		t.Fatal("backend readiness must become ready after the database schema check")
	}
}

func TestBackendDeploymentUsesSeparateReadinessProbe(t *testing.T) {
	source := readSource(t, filepath.Join("..", "..", "clusters", "kopf-k3s", "cyclops-cs", "backend-deployment.yaml"))
	if !strings.Contains(source, "readinessProbe:\n            httpGet:\n              path: /readyz") {
		t.Fatal("backend readiness probe must use /readyz")
	}
	if !strings.Contains(source, "livenessProbe:\n            httpGet:\n              path: /healthz") {
		t.Fatal("backend liveness probe must use /healthz")
	}
}
