// The makes and models a phone shop sees most, so taking a repair in is a
// tap and a pick from a list instead of typing. Anything else can still be
// typed.
export const DEVICES: Record<string, string[]> = {
  Apple: [
    'iPhone 16 Pro Max', 'iPhone 16 Pro', 'iPhone 16 Plus', 'iPhone 16', 'iPhone 16e',
    'iPhone 15 Pro Max', 'iPhone 15 Pro', 'iPhone 15 Plus', 'iPhone 15',
    'iPhone 14 Pro Max', 'iPhone 14 Pro', 'iPhone 14 Plus', 'iPhone 14',
    'iPhone 13 Pro Max', 'iPhone 13 Pro', 'iPhone 13', 'iPhone 13 mini',
    'iPhone 12 Pro Max', 'iPhone 12 Pro', 'iPhone 12', 'iPhone 12 mini',
    'iPhone 11 Pro Max', 'iPhone 11 Pro', 'iPhone 11',
    'iPhone XS Max', 'iPhone XS', 'iPhone XR', 'iPhone X',
    'iPhone SE (2022)', 'iPhone SE (2020)', 'iPhone 8 Plus', 'iPhone 8',
    'iPad', 'iPad Air', 'iPad Pro', 'iPad mini',
  ],
  Samsung: [
    'Galaxy S25 Ultra', 'Galaxy S25+', 'Galaxy S25', 'Galaxy S24 Ultra', 'Galaxy S24+', 'Galaxy S24', 'Galaxy S24 FE',
    'Galaxy S23 Ultra', 'Galaxy S23+', 'Galaxy S23', 'Galaxy S23 FE', 'Galaxy S22 Ultra', 'Galaxy S22', 'Galaxy S21 FE', 'Galaxy S21', 'Galaxy S20 FE',
    'Galaxy A56', 'Galaxy A55', 'Galaxy A54', 'Galaxy A53', 'Galaxy A52', 'Galaxy A36', 'Galaxy A35', 'Galaxy A34', 'Galaxy A33',
    'Galaxy A26', 'Galaxy A25', 'Galaxy A16', 'Galaxy A15', 'Galaxy A14', 'Galaxy A13', 'Galaxy A12', 'Galaxy A06', 'Galaxy A05s', 'Galaxy A05',
    'Galaxy Z Flip6', 'Galaxy Z Flip5', 'Galaxy Z Flip4', 'Galaxy Z Fold6', 'Galaxy Z Fold5', 'Galaxy Z Fold4',
    'Galaxy Tab A9', 'Galaxy Tab S9',
  ],
  Xiaomi: [
    'Redmi Note 14 Pro+', 'Redmi Note 14 Pro', 'Redmi Note 14', 'Redmi Note 13 Pro+', 'Redmi Note 13 Pro', 'Redmi Note 13',
    'Redmi Note 12 Pro', 'Redmi Note 12', 'Redmi Note 11', 'Redmi Note 10 Pro', 'Redmi Note 10', 'Redmi Note 9',
    'Redmi 14C', 'Redmi 13C', 'Redmi 13', 'Redmi 12C', 'Redmi 12', 'Redmi 10', 'Redmi 9A', 'Redmi 9',
    'Xiaomi 14T Pro', 'Xiaomi 14T', 'Xiaomi 14', 'Xiaomi 13T Pro', 'Xiaomi 13T', 'Xiaomi 12T', 'Xiaomi 11T',
    'POCO X7 Pro', 'POCO X6 Pro', 'POCO X6', 'POCO F6', 'POCO M6 Pro', 'POCO C65',
  ],
  Oppo: ['Reno12 Pro', 'Reno12', 'Reno11', 'Reno10', 'Reno8', 'A98', 'A80', 'A79', 'A78', 'A60', 'A58', 'A40', 'A38', 'A18', 'A17', 'Find X5'],
  Realme: ['12 Pro+', '12 Pro', '12', '11 Pro', '11', 'C67', 'C65', 'C55', 'C53', 'C35', 'C33', 'C21', 'Note 50', 'GT 6'],
  Motorola: ['Moto G85', 'Moto G84', 'Moto G75', 'Moto G55', 'Moto G54', 'Moto G35', 'Moto G34', 'Moto G24', 'Moto G14', 'Moto E14', 'Moto E13', 'Edge 50', 'Edge 40', 'Razr 50'],
  Huawei: ['P30 Pro', 'P30 Lite', 'P30', 'P40 Lite', 'P40', 'P Smart 2021', 'P Smart', 'Nova 12', 'Nova 11', 'Nova 9', 'Y9', 'Y7', 'Y6', 'Mate 20 Lite'],
  Honor: ['Magic6 Lite', 'Magic7 Lite', '200 Lite', '200', '90 Lite', '90', 'X9b', 'X8b', 'X7b', 'X6b', 'X8', 'X7', 'X6'],
  Google: ['Pixel 9 Pro', 'Pixel 9', 'Pixel 9a', 'Pixel 8 Pro', 'Pixel 8', 'Pixel 8a', 'Pixel 7 Pro', 'Pixel 7', 'Pixel 7a', 'Pixel 6 Pro', 'Pixel 6', 'Pixel 6a'],
  OnePlus: ['13', '12', '11', 'Nord 4', 'Nord CE4', 'Nord CE4 Lite', 'Nord 3', 'Nord CE3', 'Nord CE3 Lite', 'Nord 2T'],
  Vivo: ['V40', 'V30', 'Y76', 'Y36', 'Y28', 'Y22s', 'Y16', 'Y11'],
  TCL: ['50 SE', '40 SE', '40 R', '30 SE', '30', '20 SE', '505', '406'],
  Alcatel: ['1B', '1S', '3L', '1SE', '1V', '1'],
  ZTE: ['Blade A75', 'Blade A54', 'Blade A53', 'Blade A52', 'Blade A72', 'Blade V50'],
  Nokia: ['G42', 'G22', 'G21', 'C32', 'C22', 'C21', 'X20', '105', '2660 Flip'],
};

export const BRANDS = Object.keys(DEVICES);

// one tap each; they go on the resguardo, so they stay in Spanish
export const FAULTS = ['Pantalla rota', 'Batería', 'No carga', 'No enciende', 'Cámara', 'Altavoz / micrófono', 'Botones', 'Cristal trasero', 'Mojado', 'Software', 'Desbloqueo'];
