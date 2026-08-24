package database

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"testing"
)

const expectedInsertAppliedMigrationStatement = `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values ($1, $2, $3)`

func TestRuntimeDDLStaticMembershipGrantorSelection(t *testing.T) {
	tests := []struct {
		name     string
		contract staticMembershipContract
		want     string
	}{
		{
			name: "ADMIN TRUE lets PostgreSQL select the grantor",
			contract: staticMembershipContract{
				role: "k8s_query_tenant", member: "k8s_role_admin",
				admin: true, inherit: false, set: false,
			},
			want: `grant "k8s_query_tenant" to "k8s_role_admin" with admin TRUE, inherit FALSE, set FALSE`,
		},
		{
			name: "ADMIN FALSE retains the migration owner grantor",
			contract: staticMembershipContract{
				role: "k8s_query_tenant", member: "tenant_query_role",
				admin: false, inherit: true, set: false,
			},
			want: `grant "k8s_query_tenant" to "tenant_query_role" with admin FALSE, inherit TRUE, set FALSE granted by "migration_owner"`,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := staticMembershipGrantStatement(test.contract, "migration_owner"); got != test.want {
				t.Fatalf("staticMembershipGrantStatement() = %q, want %q", got, test.want)
			}
		})
	}
}

func TestMigrationAuthorityExecArguments(t *testing.T) {
	packageDir := databasePackageDir(t)
	entries, err := os.ReadDir(packageDir)
	if err != nil {
		t.Fatal(err)
	}

	var offenders []string
	for _, entry := range entries {
		name := entry.Name()
		if !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") || name == "runtime_ddl.go" {
			continue
		}
		source, err := os.ReadFile(filepath.Join(packageDir, name))
		if err != nil {
			t.Fatal(err)
		}
		offenders = append(offenders, migrationAuthorityOffenders(name, source)...)
	}
	if len(offenders) != 0 {
		t.Fatalf("runtime SQL authority escapes runtime_ddl.go:\n%s", strings.Join(offenders, "\n"))
	}
}

func TestMigrationAuthorityRejectsDDLMutations(t *testing.T) {
	tests := []struct {
		name   string
		source string
		want   string
	}{
		{
			name: "variable create index",
			source: `package database
func mutate() { statement := "CREATE INDEX sneaky_idx ON public.items (id)"; transaction.Exec(ctx, statement) }`,
			want: "must be a literal non-DDL statement",
		},
		{
			name: "arbitrary alter role superuser",
			source: `package database
func mutate() { transaction.Exec(ctx, "ALTER ROLE victim SUPERUSER") }`,
			want: "DDL statement",
		},
		{
			name: "file SQL outside migration application site",
			source: `package database
func mutate() { transaction.Exec(ctx, file.SQL) }`,
			want: "must be a literal non-DDL statement",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			offenders := migrationAuthorityOffenders("mutation.go", []byte(test.source))
			if len(offenders) != 1 || !strings.Contains(offenders[0], test.want) {
				t.Fatalf("offenders = %#v, want one containing %q", offenders, test.want)
			}
		})
	}
}

func TestMigrationAuthorityRejectsLedgerConstantBypasses(t *testing.T) {
	baseline, err := os.ReadFile(filepath.Join(databasePackageDir(t), "migrator.go"))
	if err != nil {
		t.Fatal(err)
	}
	expectedLiteral := "const insertAppliedMigrationStatement = `insert into cyclops_migrations.applied_migrations (version, filename, sha256) values ($1, $2, $3)`"
	tests := []struct {
		name   string
		source string
	}{
		{
			name: "changed package constant",
			source: strings.Replace(string(baseline), expectedLiteral,
				"const insertAppliedMigrationStatement = `delete from cyclops_migrations.applied_migrations`", 1),
		},
		{
			name:   "shadow declaration",
			source: string(baseline) + "\nfunc shadow() { const insertAppliedMigrationStatement = `select 1`; _ = insertAppliedMigrationStatement }\n",
		},
		{
			name:   "second non exec use",
			source: string(baseline) + "\nfunc reuseLedgerSQL() { _ = insertAppliedMigrationStatement }\n",
		},
		{
			name:   "second exec use",
			source: string(baseline) + "\nfunc reuseLedgerSQL() { transaction.Exec(ctx, insertAppliedMigrationStatement) }\n",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if offenders := migrationAuthorityOffenders("migrator.go", []byte(test.source)); len(offenders) == 0 {
				t.Fatal("mutation escaped migration authority contract")
			}
		})
	}
}

func TestMigrationAuthorityRejectsMigrationSQLCallBypasses(t *testing.T) {
	baseline, err := os.ReadFile(filepath.Join(databasePackageDir(t), "migrator.go"))
	if err != nil {
		t.Fatal(err)
	}
	exactCall := "transaction.Exec(ctx, file.SQL)"
	tests := []struct {
		name   string
		source string
	}{
		{
			name: "second call inside Run",
			source: strings.Replace(string(baseline),
				"\t\tif _, err := "+exactCall+"; err != nil {",
				"\t\t_, _ = "+exactCall+"\n\t\tif _, err := "+exactCall+"; err != nil {", 1),
		},
		{
			name:   "wrong receiver",
			source: strings.Replace(string(baseline), exactCall, "other.Exec(ctx, file.SQL)", 1),
		},
		{
			name:   "extra argument",
			source: strings.Replace(string(baseline), exactCall, "transaction.Exec(ctx, file.SQL, sneaky)", 1),
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if offenders := migrationAuthorityOffenders("migrator.go", []byte(test.source)); len(offenders) == 0 {
				t.Fatal("mutation escaped migration authority contract")
			}
		})
	}
}

func TestMigrationAuthorityIgnoresCommentsAndInertStrings(t *testing.T) {
	source := []byte(`package database
// transaction.Exec(ctx, "CREATE INDEX ignored ON public.items (id)")
var inert = "ALTER ROLE ignored SUPERUSER"
func mutate() { _ = "DROP TABLE ignored" }
`)
	if offenders := migrationAuthorityOffenders("mutation.go", source); len(offenders) != 0 {
		t.Fatalf("offenders = %#v, want none", offenders)
	}
}

func TestStaticRoleSettingNamesAreClosed(t *testing.T) {
	want := map[string]staticRoleSettingName{
		"default_transaction_read_only":       staticRoleSettingDefaultTransactionReadOnly,
		"statement_timeout":                   staticRoleSettingStatementTimeout,
		"idle_in_transaction_session_timeout": staticRoleSettingIdleInTransactionSessionTimeout,
	}
	if len(staticRoleSettingSQLNames) != len(want) {
		t.Fatalf("static role setting SQL name count = %d, want %d", len(staticRoleSettingSQLNames), len(want))
	}
	for sqlName, settingName := range want {
		got, ok := staticRoleSettingNameFromSQL(sqlName)
		if !ok || got != settingName {
			t.Fatalf("staticRoleSettingNameFromSQL(%q) = %v, %t; want %v, true", sqlName, got, ok, settingName)
		}
		if got.sqlName() != sqlName {
			t.Fatalf("static role setting SQL name = %q, want %q", got.sqlName(), sqlName)
		}
	}
	for _, sqlName := range []string{"search_path", "statement_timeout = 0", "unknown"} {
		if _, ok := staticRoleSettingNameFromSQL(sqlName); ok {
			t.Fatalf("staticRoleSettingNameFromSQL(%q) accepted an unapproved setting", sqlName)
		}
	}
}

func TestStaticRoleSettingsMatchRejectsUnknownActualSetting(t *testing.T) {
	desired := staticRoleSettingsContract{staticRoleSettingStatementTimeout: "20000ms"}
	actual := []staticRoleSetting{{
		values:     staticRoleSettingsContract{staticRoleSettingStatementTimeout: "20000ms"},
		hasUnknown: true,
	}}
	if staticRoleSettingsMatch(actual, desired) {
		t.Fatal("unknown actual role setting matched the closed contract")
	}
}

func TestReportingObjectKindsAndPrivilegesAreClosed(t *testing.T) {
	kinds := map[string]reportingObjectKind{
		"schema":   reportingObjectSchema,
		"relation": reportingObjectRelation,
		"sequence": reportingObjectSequence,
		"routine":  reportingObjectRoutine,
	}
	for sqlName, want := range kinds {
		got, ok := reportingObjectKindFromSQL(sqlName)
		if !ok || got != want {
			t.Fatalf("reportingObjectKindFromSQL(%q) = %v, %t; want %v, true", sqlName, got, ok, want)
		}
	}
	if _, ok := reportingObjectKindFromSQL("table; drop schema public"); ok {
		t.Fatal("accepted arbitrary reporting object kind")
	}

	valid := map[reportingObjectKind][]string{
		reportingObjectSchema:   {"USAGE", "CREATE"},
		reportingObjectRelation: {"SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"},
		reportingObjectSequence: {"USAGE", "SELECT", "UPDATE"},
		reportingObjectRoutine:  {"EXECUTE"},
	}
	if len(reportingPrivilegesByKind) != len(valid) {
		t.Fatalf("reporting privilege kind count = %d, want %d", len(reportingPrivilegesByKind), len(valid))
	}
	approvedPrivileges := map[reportingPrivilege]struct{}{}
	for kind, privileges := range valid {
		if len(reportingPrivilegesByKind[kind]) != len(privileges) {
			t.Fatalf("reporting privileges for kind %v = %d, want %d", kind, len(reportingPrivilegesByKind[kind]), len(privileges))
		}
		for _, sqlName := range privileges {
			privilege, ok := reportingPrivilegeFromSQL(kind, sqlName)
			if !ok || privilege.sqlName() != sqlName {
				t.Fatalf("reportingPrivilegeFromSQL(%v, %q) = %v, %t", kind, sqlName, privilege, ok)
			}
			approvedPrivileges[privilege] = struct{}{}
		}
		if _, ok := reportingPrivilegeFromSQL(kind, "SELECT; GRANT ALL"); ok {
			t.Fatalf("accepted arbitrary privilege for kind %v", kind)
		}
	}
	if len(reportingPrivilegeSQLNames) != len(approvedPrivileges) {
		t.Fatalf("reporting privilege SQL name count = %d, want %d", len(reportingPrivilegeSQLNames), len(approvedPrivileges))
	}
	if _, ok := reportingPrivilegeFromSQL(reportingObjectSchema, "SELECT"); ok {
		t.Fatal("accepted relation privilege for schema")
	}
}

func TestReportingObjectSQLNameQuotesSeparatedIdentifiers(t *testing.T) {
	tests := []struct {
		name   string
		object reportingObject
		want   string
	}{
		{name: "schema", object: reportingObject{kind: reportingObjectSchema, schema: `odd schema`}, want: `"odd schema"`},
		{name: "relation", object: reportingObject{kind: reportingObjectRelation, schema: `odd schema`, name: `probe.table`}, want: `"odd schema"."probe.table"`},
		{name: "sequence", object: reportingObject{kind: reportingObjectSequence, schema: `odd schema`, name: `probe.sequence`}, want: `"odd schema"."probe.sequence"`},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := reportingObjectSQLName(test.object)
			if err != nil || got != test.want {
				t.Fatalf("reportingObjectSQLName(%#v) = %q, %v; want %q, nil", test.object, got, err, test.want)
			}
		})
	}
	if _, err := reportingObjectSQLName(reportingObject{kind: reportingObjectRoutine, routineOID: 1}); err == nil {
		t.Fatal("routine OID must be canonicalized through PostgreSQL")
	}
}

func TestRuntimeDDLHasClosedMethodSet(t *testing.T) {
	source, err := os.ReadFile(filepath.Join(databasePackageDir(t), "runtime_ddl.go"))
	if err != nil {
		t.Fatal(err)
	}
	methods, err := runtimeDDLMethods(source)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{
		"ensureMigrationLedger",
		"grantReportingACL",
		"grantStaticMembership",
		"reconcileStaticRole",
		"reconcileStaticRoleSettings",
		"resetRole",
		"revokeReportingACL",
		"revokeStaticMembership",
		"setLocalRole",
		"setPassword",
	}
	if strings.Join(methods, ",") != strings.Join(want, ",") {
		t.Fatalf("runtime DDL methods = %v, want %v", methods, want)
	}
}

func TestRuntimeDDLRejectsExtraHelperOperation(t *testing.T) {
	source := []byte(`package database
 type runtimeDDL struct{}
 func (ddl runtimeDDL) ensureMigrationLedger() {}
 func (ddl runtimeDDL) grantReportingACL() {}
 func (ddl runtimeDDL) grantStaticMembership() {}
 func (ddl runtimeDDL) reconcileStaticRole() {}
 func (ddl runtimeDDL) reconcileStaticRoleSettings() {}
 func (ddl runtimeDDL) resetRole() {}
 func (ddl runtimeDDL) revokeReportingACL() {}
 func (ddl runtimeDDL) revokeStaticMembership() {}
 func (ddl runtimeDDL) setLocalRole() {}
 func (ddl runtimeDDL) setPassword() {}
 func (ddl runtimeDDL) createIndex() {}
 `)
	methods, err := runtimeDDLMethods(source)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(strings.Join(methods, ","), "createIndex") {
		t.Fatalf("methods = %v, want extra helper operation", methods)
	}
}

func databasePackageDir(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("locate database package")
	}
	return filepath.Dir(file)
}

func migrationAuthorityOffenders(filename string, source []byte) []string {
	fileSet := token.NewFileSet()
	file, err := parser.ParseFile(fileSet, filename, source, 0)
	if err != nil {
		return []string{fmt.Sprintf("parse %s: %v", filename, err)}
	}

	var offenders []string
	if filename == "migrator.go" {
		offenders = append(offenders, migratorDeclarationOffenders(fileSet, file)...)
	}
	ast.Inspect(file, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok || !isExecCall(call) || len(call.Args) < 2 {
			return true
		}
		functionName := functionNameAt(file, call.Pos())
		if isExactMigrationFileSQLCall(filename, functionName, call) || isExactLedgerInsertCall(filename, functionName, call) {
			return true
		}
		argument := call.Args[1]
		literal, ok := argument.(*ast.BasicLit)
		if !ok || literal.Kind != token.STRING {
			offenders = append(offenders, fmt.Sprintf("%s: Exec SQL must be a literal non-DDL statement, the exact ledger insert call, or the exact file.SQL migration call", fileSet.Position(argument.Pos())))
			return true
		}
		value, err := strconvUnquote(literal.Value)
		if err != nil {
			offenders = append(offenders, fmt.Sprintf("%s: invalid SQL literal", fileSet.Position(argument.Pos())))
			return true
		}
		if isDDL(value) {
			offenders = append(offenders, fmt.Sprintf("%s: DDL statement outside runtime_ddl.go", fileSet.Position(argument.Pos())))
		}
		return true
	})
	return offenders
}

func migratorDeclarationOffenders(fileSet *token.FileSet, file *ast.File) []string {
	var offenders []string
	packageDeclarations := 0
	validPackageDeclaration := false
	declarations := 0
	uses := 0
	ledgerCalls := 0
	migrationSQLReferences := 0
	migrationSQLCalls := 0

	for _, declaration := range file.Decls {
		general, ok := declaration.(*ast.GenDecl)
		if !ok || general.Tok != token.CONST {
			continue
		}
		for _, specification := range general.Specs {
			value, ok := specification.(*ast.ValueSpec)
			if !ok {
				continue
			}
			for _, name := range value.Names {
				if name.Name != "insertAppliedMigrationStatement" {
					continue
				}
				packageDeclarations++
				if len(value.Names) == 1 && len(value.Values) == 1 {
					literal, ok := value.Values[0].(*ast.BasicLit)
					if ok && literal.Kind == token.STRING {
						statement, err := strconvUnquote(literal.Value)
						validPackageDeclaration = err == nil && statement == expectedInsertAppliedMigrationStatement
					}
				}
			}
		}
	}

	ast.Inspect(file, func(node ast.Node) bool {
		switch node := node.(type) {
		case *ast.Ident:
			if node.Name != "insertAppliedMigrationStatement" {
				return true
			}
			if node.Obj != nil && node.Obj.Pos() == node.Pos() {
				declarations++
			} else {
				uses++
			}
		case *ast.SelectorExpr:
			if isFileSQLSelector(node) {
				migrationSQLReferences++
			}
		case *ast.CallExpr:
			functionName := functionNameAt(file, node.Pos())
			if isExactLedgerInsertCall("migrator.go", functionName, node) {
				ledgerCalls++
			}
			if isExactMigrationFileSQLCall("migrator.go", functionName, node) {
				migrationSQLCalls++
			}
		}
		return true
	})

	if packageDeclarations != 1 || !validPackageDeclaration {
		offenders = append(offenders, "migrator.go: insertAppliedMigrationStatement must have exactly one package-level const declaration with the exact approved DML literal")
	}
	if declarations != 1 {
		offenders = append(offenders, fmt.Sprintf("migrator.go: insertAppliedMigrationStatement declaration count = %d, want 1", declarations))
	}
	if uses != 1 || ledgerCalls != 1 {
		offenders = append(offenders, fmt.Sprintf("migrator.go: insertAppliedMigrationStatement uses = %d and exact Exec calls = %d, want 1 and 1", uses, ledgerCalls))
	}
	if migrationSQLReferences != 1 || migrationSQLCalls != 1 {
		offenders = append(offenders, fmt.Sprintf("migrator.go: file.SQL references = %d and exact Exec calls = %d, want 1 and 1", migrationSQLReferences, migrationSQLCalls))
	}
	_ = fileSet
	return offenders
}

func isExecCall(call *ast.CallExpr) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	return ok && selector.Sel.Name == "Exec"
}

func isExactMigrationFileSQLCall(filename, functionName string, call *ast.CallExpr) bool {
	if filename != "migrator.go" || functionName != "Run" || len(call.Args) != 2 || !isNamedSelectorCall(call, "transaction", "Exec") {
		return false
	}
	context, ok := call.Args[0].(*ast.Ident)
	selector, selectorOK := call.Args[1].(*ast.SelectorExpr)
	return ok && context.Name == "ctx" && selectorOK && isFileSQLSelector(selector)
}

func isExactLedgerInsertCall(filename, functionName string, call *ast.CallExpr) bool {
	if filename != "migrator.go" || functionName != "Run" || len(call.Args) != 5 || !isNamedSelectorCall(call, "transaction", "Exec") {
		return false
	}
	context, contextOK := call.Args[0].(*ast.Ident)
	statement, statementOK := call.Args[1].(*ast.Ident)
	return contextOK && context.Name == "ctx" && statementOK && statement.Name == "insertAppliedMigrationStatement" &&
		isNamedSelector(call.Args[2], "file", "Version") &&
		isNamedSelector(call.Args[3], "file", "Name") &&
		isNamedSelector(call.Args[4], "file", "SHA256")
}

func isNamedSelectorCall(call *ast.CallExpr, receiver, method string) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	identifier, ok := selector.X.(*ast.Ident)
	return ok && identifier.Name == receiver && selector.Sel.Name == method
}

func isNamedSelector(expression ast.Expr, receiver, field string) bool {
	selector, ok := expression.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	identifier, ok := selector.X.(*ast.Ident)
	return ok && identifier.Name == receiver && selector.Sel.Name == field
}

func isFileSQLSelector(selector *ast.SelectorExpr) bool {
	identifier, ok := selector.X.(*ast.Ident)
	return ok && identifier.Name == "file" && selector.Sel.Name == "SQL"
}

func functionNameAt(file *ast.File, position token.Pos) string {
	for _, declaration := range file.Decls {
		function, ok := declaration.(*ast.FuncDecl)
		if ok && function.Pos() <= position && position <= function.End() {
			return function.Name.Name
		}
	}
	return ""
}

func isDDL(statement string) bool {
	statement = strings.TrimSpace(strings.ToUpper(statement))
	for _, prefix := range []string{"CREATE ", "ALTER ", "DROP ", "GRANT ", "REVOKE ", "SET ", "RESET "} {
		if strings.HasPrefix(statement, prefix) {
			return true
		}
	}
	return false
}

func runtimeDDLMethods(source []byte) ([]string, error) {
	fileSet := token.NewFileSet()
	file, err := parser.ParseFile(fileSet, "runtime_ddl.go", source, 0)
	if err != nil {
		return nil, err
	}
	methods := []string{}
	for _, declaration := range file.Decls {
		function, ok := declaration.(*ast.FuncDecl)
		if !ok || function.Recv == nil || len(function.Recv.List) != 1 {
			continue
		}
		if receiverName(function.Recv.List[0].Type) == "runtimeDDL" {
			methods = append(methods, function.Name.Name)
		}
	}
	sort.Strings(methods)
	return methods, nil
}

func receiverName(expression ast.Expr) string {
	switch expression := expression.(type) {
	case *ast.Ident:
		return expression.Name
	case *ast.StarExpr:
		return receiverName(expression.X)
	default:
		return ""
	}
}

func strconvUnquote(value string) (string, error) {
	return strconv.Unquote(value)
}
