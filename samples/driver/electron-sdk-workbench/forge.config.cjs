const path = require('node:path');

module.exports = {
  packagerConfig: {
    name: 'Cua Driver Workbench',
    appBundleId: 'ai.cua.driver.workbench',
    appCategoryType: 'public.app-category.developer-tools',
    asar: false,
    icon: path.resolve(__dirname, '../../../libs/cua-driver/rust/scripts/CuaDriverBundle/Contents/Resources/AppIcon'),
    extendInfo: {
      NSHumanReadableCopyright: 'Copyright Cua',
    },
    ignore: [
      /^\/src($|\/)/,
      /^\/scripts($|\/)/,
      /^\/test($|\/)/,
      /^\/out($|\/)/,
    ],
  },
};
