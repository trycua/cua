#!/bin/bash

# Create a demo database file
mkdir -p ~/.sqlite3
DB_FILE="$HOME/.sqlite3/demo.db"

# Create and populate a demo database
sqlite3 "$DB_FILE" <<'EOF'
-- Create a sample table
CREATE TABLE IF NOT EXISTS employees (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    department TEXT NOT NULL,
    salary REAL NOT NULL
);

-- Insert sample data
INSERT OR IGNORE INTO employees (id, name, department, salary) VALUES
    (1, 'Alice Johnson', 'Engineering', 95000),
    (2, 'Bob Smith', 'Sales', 75000),
    (3, 'Carol White', 'Marketing', 80000),
    (4, 'David Brown', 'Engineering', 92000);

-- Display the data
SELECT 'Database Contents:' as info;
SELECT '==================' as separator;
SELECT printf('%-5s %-15s %-15s %10s', 'ID', 'Name', 'Department', 'Salary') as header;
SELECT '==================' as separator;
SELECT printf('%-5d %-15s %-15s %10.0f', id, name, department, salary) FROM employees;
SELECT '' as blank;
SELECT 'Database Information:' as info;
SELECT '==================' as separator;
SELECT 'Total Employees: ' || COUNT(*) FROM employees;
SELECT 'SQLite Version: ' || sqlite_version() AS version;
EOF

# Create a shell script that will be executed in the terminal
cat > ~/.sqlite3/interactive.sh << 'SHELL_EOF'
#!/bin/bash
echo "=== SQLite Interactive Database Shell ==="
echo "Type '.help' for help, '.quit' to exit"
echo "Database file: $HOME/.sqlite3/demo.db"
echo ""
sqlite3 "$HOME/.sqlite3/demo.db"
SHELL_EOF
chmod +x ~/.sqlite3/interactive.sh

# Launch in xfce4-terminal and keep it open
xfce4-terminal --title="SQLite Database Shell" --command="bash -c '~/.sqlite3/interactive.sh'" &

# Wait for terminal to start
sleep 3