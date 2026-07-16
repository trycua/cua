// Settings page — shows the signed-in user's identity (username and
// subject) read from the Keycloak token, with copy-to-clipboard for each.

import Box from "@cloudscape-design/components/box"
import Container from "@cloudscape-design/components/container"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import SpaceBetween from "@cloudscape-design/components/space-between"
import { userInfo } from "../auth/keycloak"

export function Settings() {
  const { sub, name } = userInfo()

  return (
    <SpaceBetween size="l">
      <Container header={<Header variant="h2">Account</Header>}>
        <SpaceBetween size="m">
          <FormField label="Username">
            {name ? (
              <CopyToClipboard
                variant="inline"
                textToCopy={name}
                textToDisplay={<code>{name}</code>}
                copyButtonAriaLabel="Copy username"
                copySuccessText="Username copied"
                copyErrorText="Failed to copy username"
              />
            ) : (
              <Box color="text-status-inactive">Unknown</Box>
            )}
          </FormField>
          <FormField label="Subject (sub)">
            {sub ? (
              <CopyToClipboard
                variant="inline"
                textToCopy={sub}
                textToDisplay={<code>{sub}</code>}
                copyButtonAriaLabel="Copy subject"
                copySuccessText="Subject copied"
                copyErrorText="Failed to copy subject"
              />
            ) : (
              <Box color="text-status-inactive">Unknown</Box>
            )}
          </FormField>
        </SpaceBetween>
      </Container>
    </SpaceBetween>
  )
}
